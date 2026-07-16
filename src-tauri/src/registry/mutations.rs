use base64::{Engine as _, engine::general_purpose::STANDARD};
use etcd_client::{Compare, CompareOp, Error as EtcdError, PutOptions, Txn, TxnOp};
use nacos_sdk::api::{config::ConfigResponse, error::Error as NacosError};
use zookeeper_client::{Acl, Acls, AuthId, CreateMode, Error as ZookeeperError, Permission};

use super::{
    AdapterId, EncodedValue, EtcdLeaseAction, EtcdLeaseActionResult, EtcdTransaction,
    EtcdTransactionResult, MutationConsistency, MutationOperation, MutationPhase, MutationResult,
    MutationValue, RegistryError, RegistryErrorCode, ResourceAddress, ResourceMutation,
    ResourceSnapshot, ValueEncoding, ZookeeperAclEntry, ZookeeperCreateMode, ZookeeperNativeAction,
    ZookeeperNativeActionResult,
    adapters::{NacosSession, read_nacos_authoritative},
};

pub(super) async fn execute_zookeeper_native_action(
    client: &zookeeper_client::Client,
    action: ZookeeperNativeAction,
    phase: &MutationPhase,
) -> Result<ZookeeperNativeActionResult, RegistryError> {
    action.validate()?;
    match action {
        ZookeeperNativeAction::SetAcl {
            address,
            expected_acl_version,
            entries,
        } => {
            let path = zookeeper_path(&address)?;
            let (previous_acls, previous_stat) = client
                .get_acl(&path)
                .await
                .map_err(|error| map_zookeeper_error("read ACL before update", error))?;
            ensure_version(
                i64::from(expected_acl_version),
                i64::from(previous_stat.aversion),
                "ZooKeeper ACL version",
            )?;
            let current_acls = entries
                .iter()
                .map(zookeeper_acl_from_entry)
                .collect::<Result<Vec<_>, _>>()?;
            let previous_entries = previous_acls.iter().map(zookeeper_acl_to_entry).collect();
            phase.mark_dispatched();
            let current_stat = client
                .set_acl(&path, &current_acls, Some(expected_acl_version))
                .await
                .map_err(|error| map_zookeeper_mutation_error("set ACL", error))?;
            Ok(ZookeeperNativeActionResult::SetAcl {
                address,
                previous_acl_version: previous_stat.aversion,
                current_acl_version: current_stat.aversion,
                previous_entries,
                current_entries: entries,
                consistency: MutationConsistency::Atomic,
            })
        }
        ZookeeperNativeAction::Create {
            address,
            value,
            mode,
        } => {
            let path = zookeeper_path(&address)?;
            let bytes = value.decoded()?;
            let parent = zookeeper_parent_path(&path)?;
            let (parent_acls, _) = client
                .get_acl(parent)
                .await
                .map_err(|error| map_zookeeper_error("read parent ACL", error))?;
            let create_mode = match mode {
                ZookeeperCreateMode::PersistentSequential => CreateMode::PersistentSequential,
                ZookeeperCreateMode::Ephemeral => CreateMode::Ephemeral,
                ZookeeperCreateMode::EphemeralSequential => CreateMode::EphemeralSequential,
            }
            .with_acls(Acls::from(parent_acls.as_slice()));
            phase.mark_dispatched();
            let (stat, sequence) = client
                .create(&path, &bytes, &create_mode)
                .await
                .map_err(|error| map_zookeeper_mutation_error("native create", error))?;
            let sequence = mode.is_sequential().then(|| sequence.to_string());
            let created_path = sequence
                .as_ref()
                .map(|sequence| format!("{path}{sequence}"))
                .unwrap_or_else(|| path.clone());
            let created_address = ResourceAddress::Zookeeper { path: created_path };
            Ok(ZookeeperNativeActionResult::Create {
                requested_address: address,
                address: created_address,
                mode,
                sequence,
                current: ResourceSnapshot::from_bytes(&bytes, Some(stat.version.to_string())),
                consistency: MutationConsistency::Atomic,
            })
        }
    }
}

fn zookeeper_acl_from_entry(entry: &ZookeeperAclEntry) -> Result<Acl, RegistryError> {
    let mut permission = Permission::NONE;
    for value in &entry.permissions {
        permission = permission
            | match value.trim().to_ascii_lowercase().as_str() {
                "read" => Permission::READ,
                "write" => Permission::WRITE,
                "create" => Permission::CREATE,
                "delete" => Permission::DELETE,
                "admin" => Permission::ADMIN,
                other => {
                    return Err(RegistryError::validation(format!(
                        "unsupported ZooKeeper ACL permission: {other}"
                    )));
                }
            };
    }
    Ok(Acl::new(
        permission,
        AuthId::new(entry.scheme.trim(), entry.id.trim()),
    ))
}

fn zookeeper_acl_to_entry(acl: &Acl) -> ZookeeperAclEntry {
    let permission = acl.permission();
    let permissions = [
        (Permission::READ, "read"),
        (Permission::WRITE, "write"),
        (Permission::CREATE, "create"),
        (Permission::DELETE, "delete"),
        (Permission::ADMIN, "admin"),
    ]
    .into_iter()
    .filter(|(required, _)| permission.has(*required))
    .map(|(_, name)| name.to_owned())
    .collect();
    ZookeeperAclEntry {
        scheme: acl.scheme().to_owned(),
        id: acl.id().to_owned(),
        permissions,
    }
}

const NACOS_WRITE_CONFIRM_ATTEMPTS: usize = 12;
const NACOS_WRITE_CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(125);

pub(super) async fn execute_etcd_lease_action(
    mut client: etcd_client::Client,
    action: EtcdLeaseAction,
    phase: &MutationPhase,
) -> Result<EtcdLeaseActionResult, RegistryError> {
    action.validate()?;
    match action {
        EtcdLeaseAction::GrantAndAttach {
            address,
            expected_version,
            ttl_seconds,
        } => {
            let key = etcd_key(&address)?;
            let expected = parse_i64_version(&expected_version, "etcd mod revision")?;
            let previous = get_bounded_etcd_value(&mut client, &key).await?;
            ensure_version(expected, previous.version, "etcd mod revision")?;
            phase.mark_dispatched();
            let grant = client
                .lease_grant(ttl_seconds, None)
                .await
                .map_err(|error| map_etcd_mutation_error("lease grant", error))?;
            let lease_id = grant.id();
            let response = client
                .txn(
                    Txn::new()
                        .when(vec![Compare::mod_revision(
                            key.clone(),
                            CompareOp::Equal,
                            expected,
                        )])
                        .and_then(vec![TxnOp::put(
                            key,
                            previous.bytes.clone(),
                            Some(PutOptions::new().with_lease(lease_id)),
                        )]),
                )
                .await
                .map_err(|error| map_etcd_mutation_error("lease attach", error))?;
            if !response.succeeded() {
                if let Err(error) = client.lease_revoke(lease_id).await {
                    return Err(RegistryError::mutation_outcome_unknown(format!(
                        "the key changed before its new lease could be attached, and cleanup of lease {lease_id} failed: {error}"
                    )));
                }
                return Err(RegistryError::conflict(
                    "etcd key changed before the new lease could be attached; the unused lease was revoked",
                ));
            }
            let version = etcd_transaction_revision(&response)?;
            Ok(EtcdLeaseActionResult::GrantAndAttach {
                address,
                lease_id: lease_id.to_string(),
                remaining_ttl_seconds: grant.ttl(),
                granted_ttl_seconds: grant.ttl(),
                previous: previous.snapshot(),
                current: ResourceSnapshot::from_bytes(&previous.bytes, Some(version)),
                consistency: MutationConsistency::Atomic,
            })
        }
        EtcdLeaseAction::Attach {
            address,
            expected_version,
            lease_id,
        } => {
            let lease_id_number = parse_i64_version(&lease_id, "etcd lease id")?;
            let lease = get_etcd_lease(&mut client, lease_id_number).await?;
            let key = etcd_key(&address)?;
            let expected = parse_i64_version(&expected_version, "etcd mod revision")?;
            let previous = get_bounded_etcd_value(&mut client, &key).await?;
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
                        .and_then(vec![TxnOp::put(
                            key,
                            previous.bytes.clone(),
                            Some(PutOptions::new().with_lease(lease_id_number)),
                        )]),
                )
                .await
                .map_err(|error| map_etcd_mutation_error("lease attach", error))?;
            if !response.succeeded() {
                return Err(RegistryError::conflict(
                    "etcd key changed before the lease could be attached",
                ));
            }
            let version = etcd_transaction_revision(&response)?;
            Ok(EtcdLeaseActionResult::Attach {
                address,
                lease_id,
                remaining_ttl_seconds: lease.ttl(),
                granted_ttl_seconds: lease.granted_ttl(),
                previous: previous.snapshot(),
                current: ResourceSnapshot::from_bytes(&previous.bytes, Some(version)),
                consistency: MutationConsistency::Atomic,
            })
        }
        EtcdLeaseAction::Detach {
            address,
            expected_version,
        } => {
            let key = etcd_key(&address)?;
            let expected = parse_i64_version(&expected_version, "etcd mod revision")?;
            let previous = get_bounded_etcd_value(&mut client, &key).await?;
            ensure_version(expected, previous.version, "etcd mod revision")?;
            if previous.lease == 0 {
                return Err(RegistryError::conflict(
                    "the selected etcd key is no longer attached to a lease",
                ));
            }
            phase.mark_dispatched();
            let response = client
                .txn(
                    Txn::new()
                        .when(vec![Compare::mod_revision(
                            key.clone(),
                            CompareOp::Equal,
                            expected,
                        )])
                        .and_then(vec![TxnOp::put(key, previous.bytes.clone(), None)]),
                )
                .await
                .map_err(|error| map_etcd_mutation_error("lease detach", error))?;
            if !response.succeeded() {
                return Err(RegistryError::conflict(
                    "etcd key changed before its lease could be detached",
                ));
            }
            let version = etcd_transaction_revision(&response)?;
            Ok(EtcdLeaseActionResult::Detach {
                address,
                previous_lease_id: previous.lease.to_string(),
                previous: previous.snapshot(),
                current: ResourceSnapshot::from_bytes(&previous.bytes, Some(version)),
                consistency: MutationConsistency::Atomic,
            })
        }
        EtcdLeaseAction::KeepAlive { address, lease_id } => {
            let lease_id_number = parse_i64_version(&lease_id, "etcd lease id")?;
            let key = etcd_key(&address)?;
            let previous = get_etcd_value(&mut client, &key).await?;
            ensure_key_lease(previous.lease, lease_id_number)?;
            phase.mark_dispatched();
            let (mut keeper, mut responses) = client
                .lease_keep_alive(lease_id_number)
                .await
                .map_err(|error| map_etcd_mutation_error("lease keep-alive", error))?;
            keeper
                .keep_alive()
                .await
                .map_err(|error| map_etcd_mutation_error("lease keep-alive", error))?;
            let response = responses
                .message()
                .await
                .map_err(|error| map_etcd_mutation_error("lease keep-alive", error))?
                .ok_or_else(|| {
                    RegistryError::mutation_outcome_unknown(
                        "etcd lease keep-alive stream ended before returning a result",
                    )
                })?;
            if response.ttl() <= 0 {
                return Err(RegistryError::not_found("etcd lease no longer exists"));
            }
            Ok(EtcdLeaseActionResult::KeepAlive {
                address,
                lease_id,
                remaining_ttl_seconds: response.ttl(),
            })
        }
        EtcdLeaseAction::Revoke {
            address,
            expected_version,
            lease_id,
        } => {
            let lease_id_number = parse_i64_version(&lease_id, "etcd lease id")?;
            let key = etcd_key(&address)?;
            let expected = parse_i64_version(&expected_version, "etcd mod revision")?;
            let previous = get_bounded_etcd_value(&mut client, &key).await?;
            ensure_version(expected, previous.version, "etcd mod revision")?;
            ensure_key_lease(previous.lease, lease_id_number)?;
            get_etcd_lease(&mut client, lease_id_number).await?;
            phase.mark_dispatched();
            client
                .lease_revoke(lease_id_number)
                .await
                .map_err(|error| map_etcd_mutation_error("lease revoke", error))?;
            Ok(EtcdLeaseActionResult::Revoke {
                address,
                lease_id,
                previous: previous.snapshot(),
                consistency: MutationConsistency::CheckedBeforeMutation,
            })
        }
    }
}

async fn get_etcd_lease(
    client: &mut etcd_client::Client,
    lease_id: i64,
) -> Result<etcd_client::LeaseTimeToLiveResponse, RegistryError> {
    let lease = client
        .lease_time_to_live(lease_id, None)
        .await
        .map_err(|error| RegistryError::network(format!("etcd lease lookup failed: {error}")))?;
    if lease.ttl() <= 0 {
        return Err(RegistryError::not_found(format!(
            "etcd lease {lease_id} does not exist"
        )));
    }
    Ok(lease)
}

fn ensure_key_lease(current: i64, expected: i64) -> Result<(), RegistryError> {
    if current == expected {
        Ok(())
    } else {
        Err(RegistryError::conflict(format!(
            "etcd key lease changed: expected {expected}, current {current}"
        )))
    }
}

pub(super) async fn execute_etcd_transaction(
    mut client: etcd_client::Client,
    transaction: EtcdTransaction,
    phase: &MutationPhase,
) -> Result<EtcdTransactionResult, RegistryError> {
    transaction.validate()?;
    let mut compares = Vec::with_capacity(transaction.mutations.len());
    let mut operations = Vec::with_capacity(transaction.mutations.len());
    let mut prepared = Vec::with_capacity(transaction.mutations.len());

    for mutation in transaction.mutations {
        match mutation {
            ResourceMutation::Create { address, value, .. } => {
                let key = etcd_key(&address)?;
                let bytes = value.decoded()?;
                compares.push(Compare::version(key.clone(), CompareOp::Equal, 0));
                operations.push(TxnOp::put(key, bytes.clone(), None));
                prepared.push(PreparedEtcdTransaction::Create { address, bytes });
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
                compares.push(Compare::mod_revision(
                    key.clone(),
                    CompareOp::Equal,
                    expected,
                ));
                operations.push(TxnOp::put(key, bytes.clone(), options));
                prepared.push(PreparedEtcdTransaction::Update {
                    address,
                    previous,
                    bytes,
                });
            }
            ResourceMutation::Delete {
                address,
                expected_version,
            } => {
                let key = etcd_key(&address)?;
                let expected = parse_i64_version(&expected_version, "etcd mod revision")?;
                let previous = get_etcd_value(&mut client, &key).await?;
                ensure_version(expected, previous.version, "etcd mod revision")?;
                compares.push(Compare::mod_revision(
                    key.clone(),
                    CompareOp::Equal,
                    expected,
                ));
                operations.push(TxnOp::delete(key, None));
                prepared.push(PreparedEtcdTransaction::Delete { address, previous });
            }
        }
    }

    phase.mark_dispatched();
    let response = client
        .txn(Txn::new().when(compares).and_then(operations))
        .await
        .map_err(|error| map_etcd_mutation_error("transaction", error))?;
    if !response.succeeded() {
        return Err(RegistryError::conflict(
            "one or more etcd transaction compares failed; no operations were applied",
        ));
    }
    let revision = etcd_transaction_revision(&response)?;
    let results = prepared
        .into_iter()
        .map(|item| item.into_result(&revision))
        .collect();
    Ok(EtcdTransactionResult { revision, results })
}

enum PreparedEtcdTransaction {
    Create {
        address: ResourceAddress,
        bytes: Vec<u8>,
    },
    Update {
        address: ResourceAddress,
        previous: EtcdValue,
        bytes: Vec<u8>,
    },
    Delete {
        address: ResourceAddress,
        previous: EtcdValue,
    },
}

impl PreparedEtcdTransaction {
    fn into_result(self, revision: &str) -> MutationResult {
        match self {
            Self::Create { address, bytes } => MutationResult {
                operation: MutationOperation::Create,
                address,
                previous: None,
                current: Some(ResourceSnapshot::from_bytes(
                    &bytes,
                    Some(revision.to_owned()),
                )),
                consistency: MutationConsistency::Atomic,
            },
            Self::Update {
                address,
                previous,
                bytes,
            } => MutationResult {
                operation: MutationOperation::Update,
                address,
                previous: Some(previous.snapshot()),
                current: Some(ResourceSnapshot::from_bytes(
                    &bytes,
                    Some(revision.to_owned()),
                )),
                consistency: MutationConsistency::Atomic,
            },
            Self::Delete { address, previous } => MutationResult {
                operation: MutationOperation::Delete,
                address,
                previous: Some(previous.snapshot()),
                current: None,
                consistency: MutationConsistency::Atomic,
            },
        }
    }
}

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
                .publish_config(
                    data_id.clone(),
                    group.clone(),
                    content.clone(),
                    content_type,
                )
                .await
                .map_err(|error| map_nacos_mutation_error("create", error))?;
            if !published {
                return Err(RegistryError::invalid_response(
                    "Nacos create returned false",
                ));
            }
            let current = confirm_nacos_config_content(session, &data_id, &group, &content)
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
            let content = nacos_content(value)?;
            phase.mark_dispatched();
            let published = match session
                .config
                .publish_config_cas(
                    data_id.clone(),
                    group.clone(),
                    content.clone(),
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
            let current = confirm_nacos_config_content(session, &data_id, &group, &content)
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

async fn get_bounded_etcd_value(
    client: &mut etcd_client::Client,
    key: &[u8],
) -> Result<EtcdValue, RegistryError> {
    let value = get_etcd_value(client, key).await?;
    EncodedValue::try_from_inline_bytes(&value.bytes)?;
    Ok(value)
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
    read_nacos_authoritative(session, data_id, group).await
}

async fn confirm_nacos_config_content(
    session: &NacosSession,
    data_id: &str,
    group: &str,
    expected_content: &str,
) -> Result<ConfigResponse, RegistryError> {
    for attempt in 0..NACOS_WRITE_CONFIRM_ATTEMPTS {
        match read_nacos_authoritative(session, data_id, group).await {
            Ok(response) if response.content() == expected_content => return Ok(response),
            Ok(_) => {}
            Err(error) if error.code == RegistryErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        if attempt + 1 < NACOS_WRITE_CONFIRM_ATTEMPTS {
            tokio::time::sleep(NACOS_WRITE_CONFIRM_DELAY).await;
        }
    }
    Err(RegistryError::not_found(
        "Nacos config change did not become visible before the confirmation deadline",
    ))
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
