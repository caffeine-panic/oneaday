use base64::{Engine as _, engine::general_purpose::STANDARD};
use etcd_client::{Compare, CompareOp, Error as EtcdError, PutOptions, Txn, TxnOp};
use nacos_sdk::api::{config::ConfigResponse, error::Error as NacosError};
use zookeeper_client::{Acls, CreateMode, Error as ZookeeperError};

use super::{
    AdapterId, MutationConsistency, MutationOperation, MutationPhase, MutationResult,
    MutationValue, RegistryError, ResourceAddress, ResourceMutation, ResourceSnapshot,
    ValueEncoding, adapters::NacosSession,
};

pub(super) async fn mutate_etcd(
    mut client: etcd_client::Client,
    mutation: ResourceMutation,
    phase: &MutationPhase,
) -> Result<MutationResult, RegistryError> {
    match mutation {
        ResourceMutation::Create { address, value, .. } => {
            let key = etcd_key(&address)?;
            let bytes = value.decoded()?;
            phase.mark_dispatched();
            let response = client
                .txn(
                    Txn::new()
                        .when(vec![Compare::version(key.clone(), CompareOp::Equal, 0)])
                        .and_then(vec![TxnOp::put(key, bytes.clone(), None)]),
                )
                .await
                .map_err(|error| map_etcd_mutation_error("create", error))?;
            if !response.succeeded() {
                return Err(RegistryError::conflict("etcd key already exists"));
            }
            let version = etcd_transaction_revision(&response)?;
            Ok(MutationResult {
                operation: MutationOperation::Create,
                address,
                previous: None,
                current: Some(ResourceSnapshot::from_bytes(&bytes, Some(version))),
                consistency: MutationConsistency::Atomic,
            })
        }
        ResourceMutation::Update {
            address,
            value,
            expected_version,
            ..
        } => {
            let key = etcd_key(&address)?;
            let expected = parse_i64_version(&expected_version, "etcd mod revision")?;
            let previous = get_etcd_value(&mut client, &key).await?;
            ensure_version(expected, previous.version, "etcd mod revision")?;
            let bytes = value.decoded()?;
            let options =
                (previous.lease != 0).then(|| PutOptions::new().with_lease(previous.lease));
            phase.mark_dispatched();
            let response = client
                .txn(
                    Txn::new()
                        .when(vec![Compare::mod_revision(
                            key.clone(),
                            CompareOp::Equal,
                            expected,
                        )])
                        .and_then(vec![TxnOp::put(key, bytes.clone(), options)]),
                )
                .await
                .map_err(|error| map_etcd_mutation_error("update", error))?;
            if !response.succeeded() {
                return Err(RegistryError::conflict(
                    "etcd key changed after it was loaded; refresh before saving",
                ));
            }
            let version = etcd_transaction_revision(&response)?;
            Ok(MutationResult {
                operation: MutationOperation::Update,
                address,
                previous: Some(previous.snapshot()),
                current: Some(ResourceSnapshot::from_bytes(&bytes, Some(version))),
                consistency: MutationConsistency::Atomic,
            })
        }
        ResourceMutation::Delete {
            address,
            expected_version,
        } => {
            let key = etcd_key(&address)?;
            let expected = parse_i64_version(&expected_version, "etcd mod revision")?;
            let previous = get_etcd_value(&mut client, &key).await?;
            ensure_version(expected, previous.version, "etcd mod revision")?;
            phase.mark_dispatched();
            let response = client
                .txn(
                    Txn::new()
                        .when(vec![Compare::mod_revision(
                            key.clone(),
                            CompareOp::Equal,
                            expected,
                        )])
                        .and_then(vec![TxnOp::delete(key, None)]),
                )
                .await
                .map_err(|error| map_etcd_mutation_error("delete", error))?;
            if !response.succeeded() {
                return Err(RegistryError::conflict(
                    "etcd key changed after it was loaded; refresh before deleting",
                ));
            }
            Ok(MutationResult {
                operation: MutationOperation::Delete,
                address,
                previous: Some(previous.snapshot()),
                current: None,
                consistency: MutationConsistency::Atomic,
            })
        }
    }
}

pub(super) async fn mutate_zookeeper(
    client: &zookeeper_client::Client,
    mutation: ResourceMutation,
    phase: &MutationPhase,
) -> Result<MutationResult, RegistryError> {
    match mutation {
        ResourceMutation::Create { address, value, .. } => {
            let path = zookeeper_path(&address)?;
            let bytes = value.decoded()?;
            let parent = zookeeper_parent_path(&path)?;
            let (parent_acls, _) = client
                .get_acl(parent)
                .await
                .map_err(|error| map_zookeeper_error("read parent ACL", error))?;
            let create_mode = CreateMode::Persistent.with_acls(Acls::from(parent_acls.as_slice()));
            phase.mark_dispatched();
            client
                .create(&path, &bytes, &create_mode)
                .await
                .map_err(|error| map_zookeeper_mutation_error("create", error))?;
            Ok(MutationResult {
                operation: MutationOperation::Create,
                address,
                previous: None,
                current: Some(ResourceSnapshot::from_bytes(&bytes, Some("0".to_owned()))),
                consistency: MutationConsistency::Atomic,
            })
        }
        ResourceMutation::Update {
            address,
            value,
            expected_version,
            ..
        } => {
            let path = zookeeper_path(&address)?;
            let expected = parse_i32_version(&expected_version, "ZooKeeper version")?;
            let (previous_bytes, previous_stat) = client
                .get_data(&path)
                .await
                .map_err(|error| map_zookeeper_error("read before update", error))?;
            ensure_version(
                i64::from(expected),
                i64::from(previous_stat.version),
                "ZooKeeper version",
            )?;
            let bytes = value.decoded()?;
            phase.mark_dispatched();
            let current_stat = client
                .set_data(&path, &bytes, Some(expected))
                .await
                .map_err(|error| map_zookeeper_mutation_error("update", error))?;
            Ok(MutationResult {
                operation: MutationOperation::Update,
                address,
                previous: Some(ResourceSnapshot::from_bytes(
                    &previous_bytes,
                    Some(previous_stat.version.to_string()),
                )),
                current: Some(ResourceSnapshot::from_bytes(
                    &bytes,
                    Some(current_stat.version.to_string()),
                )),
                consistency: MutationConsistency::Atomic,
            })
        }
        ResourceMutation::Delete {
            address,
            expected_version,
        } => {
            let path = zookeeper_path(&address)?;
            let expected = parse_i32_version(&expected_version, "ZooKeeper version")?;
            let (previous_bytes, previous_stat) = client
                .get_data(&path)
                .await
                .map_err(|error| map_zookeeper_error("read before delete", error))?;
            ensure_version(
                i64::from(expected),
                i64::from(previous_stat.version),
                "ZooKeeper version",
            )?;
            phase.mark_dispatched();
            client
                .delete(&path, Some(expected))
                .await
                .map_err(|error| map_zookeeper_mutation_error("delete", error))?;
            Ok(MutationResult {
                operation: MutationOperation::Delete,
                address,
                previous: Some(ResourceSnapshot::from_bytes(
                    &previous_bytes,
                    Some(previous_stat.version.to_string()),
                )),
                current: None,
                consistency: MutationConsistency::Atomic,
            })
        }
    }
}

pub(super) async fn mutate_nacos(
    session: &NacosSession,
    mutation: ResourceMutation,
    phase: &MutationPhase,
) -> Result<MutationResult, RegistryError> {
    match mutation {
        ResourceMutation::Create {
            address,
            value,
            content_type,
        } => {
            let (group, data_id) = nacos_identity(&address)?;
            match session
                .config
                .get_config(data_id.clone(), group.clone())
                .await
            {
                Ok(_) => return Err(RegistryError::conflict("Nacos config already exists")),
                Err(NacosError::ConfigNotFound(_)) => {}
                Err(error) => return Err(map_nacos_error("read before create", error)),
            }
            let content = nacos_content(value)?;
            phase.mark_dispatched();
            let published = session
                .config
                .publish_config(data_id.clone(), group.clone(), content, content_type)
                .await
                .map_err(|error| map_nacos_mutation_error("create", error))?;
            if !published {
                return Err(RegistryError::invalid_response(
                    "Nacos create returned false",
                ));
            }
            let current = get_nacos_config(session, &data_id, &group)
                .await
                .map_err(|error| {
                    RegistryError::mutation_outcome_unknown(format!(
                        "Nacos create was accepted, but its remote result could not be read: {}",
                        error.message
                    ))
                })?;
            Ok(MutationResult {
                operation: MutationOperation::Create,
                address,
                previous: None,
                current: Some(nacos_snapshot(&current)),
                consistency: MutationConsistency::CheckedBeforeMutation,
            })
        }
        ResourceMutation::Update {
            address,
            value,
            content_type,
            expected_version,
        } => {
            let (group, data_id) = nacos_identity(&address)?;
            let previous = get_nacos_config(session, &data_id, &group).await?;
            ensure_text_version(&expected_version, previous.md5(), "Nacos MD5")?;
            let content_type = content_type.or_else(|| Some(previous.content_type().clone()));
            phase.mark_dispatched();
            let published = match session
                .config
                .publish_config_cas(
                    data_id.clone(),
                    group.clone(),
                    nacos_content(value)?,
                    content_type,
                    expected_version.clone(),
                )
                .await
            {
                Ok(published) => published,
                Err(error) => {
                    if let NacosError::ErrResult(message) = &error
                        && is_nacos_publish_rejection(message)
                        && let Ok(current) = get_nacos_config(session, &data_id, &group).await
                        && current.md5() != expected_version.trim()
                    {
                        return Err(RegistryError::conflict(
                            "Nacos config changed after it was loaded; refresh before saving",
                        ));
                    }
                    return Err(map_nacos_cas_error(error));
                }
            };
            if !published {
                return Err(RegistryError::conflict(
                    "Nacos config changed after it was loaded; refresh before saving",
                ));
            }
            let current = get_nacos_config(session, &data_id, &group)
                .await
                .map_err(|error| {
                    RegistryError::mutation_outcome_unknown(format!(
                        "Nacos conditional update was accepted, but its remote result could not be read: {}",
                        error.message
                    ))
                })?;
            Ok(MutationResult {
                operation: MutationOperation::Update,
                address,
                previous: Some(nacos_snapshot(&previous)),
                current: Some(nacos_snapshot(&current)),
                consistency: MutationConsistency::Atomic,
            })
        }
        ResourceMutation::Delete {
            address,
            expected_version,
        } => {
            let (group, data_id) = nacos_identity(&address)?;
            let previous = get_nacos_config(session, &data_id, &group).await?;
            ensure_text_version(&expected_version, previous.md5(), "Nacos MD5")?;
            phase.mark_dispatched();
            let removed = session
                .config
                .remove_config(data_id, group)
                .await
                .map_err(|error| map_nacos_mutation_error("delete", error))?;
            if !removed {
                return Err(RegistryError::invalid_response(
                    "Nacos delete returned false",
                ));
            }
            Ok(MutationResult {
                operation: MutationOperation::Delete,
                address,
                previous: Some(nacos_snapshot(&previous)),
                current: None,
                consistency: MutationConsistency::CheckedBeforeMutation,
            })
        }
    }
}

struct EtcdValue {
    bytes: Vec<u8>,
    version: i64,
    lease: i64,
}

impl EtcdValue {
    fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot::from_bytes(&self.bytes, Some(self.version.to_string()))
    }
}

async fn get_etcd_value(
    client: &mut etcd_client::Client,
    key: &[u8],
) -> Result<EtcdValue, RegistryError> {
    let response = client
        .get(key, None)
        .await
        .map_err(|error| RegistryError::network(format!("etcd read failed: {error}")))?;
    let value = response
        .kvs()
        .first()
        .ok_or_else(|| RegistryError::not_found("etcd key does not exist"))?;
    Ok(EtcdValue {
        bytes: value.value().to_vec(),
        version: value.mod_revision(),
        lease: value.lease(),
    })
}

fn etcd_key(address: &ResourceAddress) -> Result<Vec<u8>, RegistryError> {
    match address {
        ResourceAddress::Etcd { key_base64 } => STANDARD
            .decode(key_base64)
            .map_err(|_| RegistryError::validation("etcd key is not valid base64")),
        _ => Err(adapter_mismatch(AdapterId::Etcd, address)),
    }
}

fn etcd_transaction_revision(response: &etcd_client::TxnResponse) -> Result<String, RegistryError> {
    response
        .header()
        .map(|header| header.revision().to_string())
        .ok_or_else(|| {
            RegistryError::mutation_outcome_unknown(
                "etcd transaction succeeded but returned no revision; refresh the key before retrying",
            )
        })
}

fn map_etcd_mutation_error(operation: &str, error: EtcdError) -> RegistryError {
    match error {
        EtcdError::InvalidArgs(message) => {
            RegistryError::validation(format!("etcd {operation} request is invalid: {message}"))
        }
        EtcdError::GRpcStatus(status) => match status.code() {
            tonic::Code::InvalidArgument | tonic::Code::OutOfRange => RegistryError::validation(
                format!("etcd {operation} request was rejected: {status}"),
            ),
            tonic::Code::PermissionDenied | tonic::Code::Unauthenticated => {
                RegistryError::permission_denied(format!(
                    "etcd {operation} is not authorized: {status}"
                ))
            }
            tonic::Code::AlreadyExists | tonic::Code::Aborted | tonic::Code::FailedPrecondition => {
                RegistryError::conflict(format!(
                    "etcd {operation} was rejected by a state conflict: {status}"
                ))
            }
            tonic::Code::NotFound => RegistryError::not_found(format!(
                "etcd {operation} target does not exist: {status}"
            )),
            tonic::Code::ResourceExhausted => RegistryError::resource_exhausted(format!(
                "etcd {operation} was rejected because the server is resource constrained: {status}"
            )),
            tonic::Code::Unimplemented => RegistryError::unsupported(format!(
                "etcd server does not support {operation}: {status}"
            )),
            _ => RegistryError::mutation_outcome_unknown(format!(
                "etcd {operation} returned an ambiguous gRPC status after write dispatch; refresh the key before retrying: {status}"
            )),
        },
        other => RegistryError::mutation_outcome_unknown(format!(
            "etcd {operation} returned an error after write dispatch; refresh the key before retrying: {other}"
        )),
    }
}

fn zookeeper_path(address: &ResourceAddress) -> Result<String, RegistryError> {
    match address {
        ResourceAddress::Zookeeper { path } => Ok(path.clone()),
        _ => Err(adapter_mismatch(AdapterId::Zookeeper, address)),
    }
}

fn zookeeper_parent_path(path: &str) -> Result<&str, RegistryError> {
    match path.rfind('/') {
        Some(0) => Ok("/"),
        Some(index) => Ok(&path[..index]),
        None => Err(RegistryError::validation(
            "ZooKeeper path must be absolute before resolving its parent",
        )),
    }
}

fn map_zookeeper_error(operation: &str, error: ZookeeperError) -> RegistryError {
    match error {
        ZookeeperError::NoNode => RegistryError::not_found(format!(
            "ZooKeeper {operation} failed because the znode or its parent does not exist"
        )),
        ZookeeperError::BadVersion => RegistryError::conflict(format!(
            "ZooKeeper {operation} failed because the znode version changed"
        )),
        ZookeeperError::NodeExists => RegistryError::conflict("ZooKeeper znode already exists"),
        ZookeeperError::NotEmpty => RegistryError::conflict(
            "ZooKeeper znode has children; recursive delete requires a separate confirmed action",
        ),
        ZookeeperError::NoChildrenForEphemerals => {
            RegistryError::conflict("ZooKeeper cannot create a child under an ephemeral znode")
        }
        ZookeeperError::QuotaExceeded => RegistryError::conflict("ZooKeeper path quota exceeded"),
        ZookeeperError::BadArguments(_) | ZookeeperError::InvalidAcl => {
            RegistryError::validation(format!("ZooKeeper {operation} request is invalid: {error}"))
        }
        ZookeeperError::NoAuth | ZookeeperError::AuthFailed => RegistryError::permission_denied(
            format!("ZooKeeper {operation} is not authorized: {error}"),
        ),
        ZookeeperError::Unimplemented => RegistryError::unsupported(format!(
            "ZooKeeper server does not support {operation}: {error}"
        )),
        other => RegistryError::network(format!("ZooKeeper {operation} failed: {other}")),
    }
}

fn map_zookeeper_mutation_error(operation: &str, error: ZookeeperError) -> RegistryError {
    match error {
        known @ (ZookeeperError::NoNode
        | ZookeeperError::BadVersion
        | ZookeeperError::NodeExists
        | ZookeeperError::NotEmpty
        | ZookeeperError::NoChildrenForEphemerals
        | ZookeeperError::QuotaExceeded
        | ZookeeperError::NoAuth
        | ZookeeperError::AuthFailed
        | ZookeeperError::InvalidAcl
        | ZookeeperError::Unimplemented) => map_zookeeper_error(operation, known),
        known @ ZookeeperError::BadArguments(_) => map_zookeeper_error(operation, known),
        other => RegistryError::mutation_outcome_unknown(format!(
            "ZooKeeper {operation} returned an error after write dispatch; refresh the znode before retrying: {other}"
        )),
    }
}

fn nacos_identity(address: &ResourceAddress) -> Result<(String, String), RegistryError> {
    match address {
        ResourceAddress::NacosConfig { group, data_id } => Ok((group.clone(), data_id.clone())),
        _ => Err(adapter_mismatch(AdapterId::Nacos, address)),
    }
}

fn nacos_content(value: MutationValue) -> Result<String, RegistryError> {
    if value.encoding != ValueEncoding::Utf8 {
        return Err(RegistryError::unsupported(
            "Nacos configuration values must be UTF-8 text",
        ));
    }
    if value.content.is_empty() {
        return Err(RegistryError::validation(
            "the current Nacos SDK does not publish empty configuration values",
        ));
    }
    Ok(value.content)
}

async fn get_nacos_config(
    session: &NacosSession,
    data_id: &str,
    group: &str,
) -> Result<ConfigResponse, RegistryError> {
    session
        .config
        .get_config(data_id.to_owned(), group.to_owned())
        .await
        .map_err(|error| map_nacos_error("read", error))
}

fn nacos_snapshot(response: &ConfigResponse) -> ResourceSnapshot {
    ResourceSnapshot::from_bytes(response.content().as_bytes(), Some(response.md5().clone()))
}

fn map_nacos_error(operation: &str, error: NacosError) -> RegistryError {
    match error {
        NacosError::ConfigNotFound(_) => RegistryError::not_found("Nacos config does not exist"),
        NacosError::ConfigQueryConflict(_) => RegistryError::conflict(format!(
            "Nacos {operation} conflicted with another configuration change"
        )),
        other => RegistryError::network(format!("Nacos {operation} failed: {other}")),
    }
}

fn map_nacos_cas_error(error: NacosError) -> RegistryError {
    match error {
        NacosError::ConfigQueryConflict(_) => RegistryError::conflict(
            "Nacos config changed after it was loaded; refresh before saving",
        ),
        NacosError::ErrResult(message) if is_nacos_publish_rejection(&message) => {
            map_nacos_publish_rejection("conditional update", message)
        }
        other => map_nacos_mutation_error("conditional update", other),
    }
}

fn is_nacos_publish_rejection(message: &str) -> bool {
    message.starts_with("handle publish_config failed:")
}

fn map_nacos_publish_rejection(operation: &str, message: String) -> RegistryError {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("error_code=403")
        || normalized.contains("forbidden")
        || normalized.contains("unauthorized")
        || normalized.contains("permission")
    {
        RegistryError::permission_denied(format!(
            "Nacos {operation} was rejected by the server: {message}"
        ))
    } else {
        RegistryError::invalid_response(format!(
            "Nacos {operation} was rejected by the server: {message}"
        ))
    }
}

fn map_nacos_mutation_error(operation: &str, error: NacosError) -> RegistryError {
    match error {
        NacosError::InvalidParam(parameter, message) => RegistryError::validation(format!(
            "Nacos {operation} parameter '{parameter}' is invalid: {message}"
        )),
        NacosError::Serialization(error) => RegistryError::invalid_response(format!(
            "Nacos {operation} request could not be encoded: {error}"
        )),
        NacosError::ErrResult(message) if is_nacos_publish_rejection(&message) => {
            map_nacos_publish_rejection(operation, message)
        }
        other => RegistryError::mutation_outcome_unknown(format!(
            "Nacos {operation} returned an error after write dispatch; refresh the config before retrying: {other}"
        )),
    }
}

fn parse_i64_version(value: &str, label: &str) -> Result<i64, RegistryError> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|version| *version >= 0)
        .ok_or_else(|| RegistryError::validation(format!("{label} is invalid")))
}

fn parse_i32_version(value: &str, label: &str) -> Result<i32, RegistryError> {
    value
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|version| *version >= 0)
        .ok_or_else(|| RegistryError::validation(format!("{label} is invalid")))
}

fn ensure_version(expected: i64, current: i64, label: &str) -> Result<(), RegistryError> {
    if expected == current {
        Ok(())
    } else {
        Err(RegistryError::conflict(format!(
            "{label} changed: expected {expected}, current {current}"
        )))
    }
}

fn ensure_text_version(expected: &str, current: &str, label: &str) -> Result<(), RegistryError> {
    if expected.trim() == current {
        Ok(())
    } else {
        Err(RegistryError::conflict(format!(
            "{label} changed: expected {}, current {current}",
            expected.trim()
        )))
    }
}

fn adapter_mismatch(adapter: AdapterId, address: &ResourceAddress) -> RegistryError {
    RegistryError::validation(format!(
        "resource address {address:?} does not belong to {adapter:?}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryErrorCode;

    #[test]
    fn nacos_cas_server_rejection_is_not_blindly_reported_as_a_conflict() {
        let error = map_nacos_cas_error(NacosError::ErrResult(
            "handle publish_config failed: result_code=500".to_owned(),
        ));

        assert_eq!(error.code, RegistryErrorCode::InvalidResponse);
    }

    #[test]
    fn zookeeper_create_uses_the_parent_path_for_acl_inheritance() {
        assert_eq!(zookeeper_parent_path("/child").unwrap(), "/");
        assert_eq!(zookeeper_parent_path("/apps/payment").unwrap(), "/apps");
    }

    #[test]
    fn zookeeper_authorization_failures_are_not_network_errors() {
        let error = map_zookeeper_error("create", ZookeeperError::NoAuth);

        assert_eq!(error.code, RegistryErrorCode::PermissionDenied);
        assert!(!error.retryable);
    }

    #[test]
    fn zookeeper_transport_failure_after_dispatch_has_an_unknown_outcome() {
        let error = map_zookeeper_mutation_error("update", ZookeeperError::ConnectionLoss);

        assert_eq!(error.code, RegistryErrorCode::OutcomeUnknown);
        assert!(!error.retryable);
    }

    #[test]
    fn nacos_transport_failure_after_dispatch_has_an_unknown_outcome() {
        let error = map_nacos_mutation_error(
            "update",
            NacosError::ErrResult("the connection is not connected".to_owned()),
        );

        assert_eq!(error.code, RegistryErrorCode::OutcomeUnknown);
        assert!(!error.retryable);
    }

    #[test]
    fn etcd_permission_rejection_is_structured() {
        let error = map_etcd_mutation_error(
            "update",
            EtcdError::GRpcStatus(tonic::Status::permission_denied("denied")),
        );

        assert_eq!(error.code, RegistryErrorCode::PermissionDenied);
        assert!(!error.retryable);
    }
}
