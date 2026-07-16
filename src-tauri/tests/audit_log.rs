use atlas_registry_lib::{
    audit::{AuditHistoryKind, AuditLog, NativeAuditOperation},
    registry::{
        MutationConsistency, MutationOperation, MutationResult, MutationValue, ResourceAddress,
        ResourceMutation, ResourceSnapshot, ValueEncoding,
    },
};

#[test]
fn audit_log_persists_started_and_applied_events_without_resource_values() {
    tauri::async_runtime::block_on(async {
        let directory = std::env::temp_dir().join(format!(
            "atlas-audit-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        let address = ResourceAddress::Zookeeper {
            path: "/apps/payment".to_owned(),
        };
        let mutation = ResourceMutation::Update {
            address: address.clone(),
            value: MutationValue {
                content: "TOP_SECRET_VALUE".to_owned(),
                encoding: ValueEncoding::Utf8,
            },
            content_type: Some("text".to_owned()),
            expected_version: "3".to_owned(),
        };
        let previous = ResourceSnapshot::from_bytes(b"old secret", Some("3".to_owned()));
        let result = MutationResult {
            operation: MutationOperation::Update,
            address,
            previous: Some(previous.clone()),
            current: Some(ResourceSnapshot::from_bytes(
                b"TOP_SECRET_VALUE",
                Some("4".to_owned()),
            )),
            consistency: MutationConsistency::Atomic,
        };
        let log = AuditLog::default();

        log.record_started_in(
            &directory,
            "connection-1",
            "operation-1",
            &mutation,
            Some(&previous),
        )
        .await
        .expect("started event should be written");
        log.record_applied_in(&directory, "connection-1", "operation-1", &result)
            .await
            .expect("applied event should be written");
        log.record_outcome_unknown_in(&directory, "connection-1", "operation-2")
            .await
            .expect("unknown outcome event should be written");
        log.record_native_started_in(
            &directory,
            "connection-1",
            "operation-3",
            NativeAuditOperation::EtcdLeaseKeepAlive,
            Some(&ResourceAddress::Etcd {
                key_base64: "L2F0bGFzL2tleQ==".to_owned(),
            }),
            None,
            None,
            Some("lease:9223372036854775807"),
        )
        .await
        .expect("native started event should be written");
        log.record_native_applied_in(
            &directory,
            "connection-1",
            "operation-3",
            NativeAuditOperation::EtcdLeaseKeepAlive,
            Some(&ResourceAddress::Etcd {
                key_base64: "L2F0bGFzL2tleQ==".to_owned(),
            }),
            None,
            None,
            MutationConsistency::CheckedBeforeMutation,
            Some("lease:9223372036854775807"),
        )
        .await
        .expect("native applied event should be written");

        let content = tokio::fs::read_to_string(directory.join("mutation-audit.jsonl"))
            .await
            .expect("audit file should exist");
        assert_eq!(content.lines().count(), 5);
        assert!(content.contains("mutationStarted"));
        assert!(content.contains("mutationApplied"));
        assert!(content.contains("mutationOutcomeUnknown"));
        assert!(content.contains("nativeMutationStarted"));
        assert!(content.contains("nativeMutationApplied"));
        assert!(content.contains("etcdLeaseKeepAlive"));
        assert!(content.lines().next().unwrap().contains(&previous.sha256));
        assert!(content.contains(&result.current.expect("current snapshot").sha256));
        assert!(!content.contains("TOP_SECRET_VALUE"));
        assert!(!content.contains("old secret"));

        let first_page = log
            .load_recent_in(&directory, Some("connection-1"), None, 2)
            .await
            .expect("recent audit events should load");
        assert_eq!(first_page.items.len(), 2);
        assert_eq!(first_page.items[0].kind, AuditHistoryKind::Applied);
        assert_eq!(
            first_page.items[0].native_operation,
            Some(NativeAuditOperation::EtcdLeaseKeepAlive)
        );
        assert_eq!(
            first_page.items[0].native_target.as_deref(),
            Some("lease:9223372036854775807")
        );
        assert_eq!(first_page.items[1].kind, AuditHistoryKind::Started);
        assert!(first_page.next_cursor.is_some());

        let second_page = log
            .load_recent_in(
                &directory,
                Some("connection-1"),
                first_page.next_cursor.clone(),
                2,
            )
            .await
            .expect("older audit events should load by cursor");
        assert_eq!(second_page.items.len(), 2);
        assert_eq!(second_page.items[0].kind, AuditHistoryKind::OutcomeUnknown);
        assert_eq!(second_page.items[1].kind, AuditHistoryKind::Applied);
        assert!(second_page.next_cursor.is_some());

        let third_page = log
            .load_recent_in(
                &directory,
                Some("connection-1"),
                second_page.next_cursor.clone(),
                2,
            )
            .await
            .expect("oldest audit event should load by cursor");
        assert_eq!(third_page.items.len(), 1);
        assert_eq!(third_page.items[0].kind, AuditHistoryKind::Started);
        assert!(third_page.next_cursor.is_none());

        let serialized = serde_json::to_string(&first_page).expect("history page should serialize");
        assert!(!serialized.contains("TOP_SECRET_VALUE"));
        assert!(!serialized.contains("old secret"));

        tokio::fs::remove_dir_all(directory)
            .await
            .expect("test directory should be removable");
    });
}
