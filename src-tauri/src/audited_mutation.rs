use std::{future::Future, path::Path};

use crate::{
    audit::AuditLog,
    registry::{MutationPhase, OperationId, RegistryError, RegistryErrorCode, RegistryService},
};

pub(crate) async fn run<T, Run, RunFuture>(
    directory: &Path,
    service: &RegistryService,
    audit: &AuditLog,
    connection_id: &str,
    operation_id: &str,
    run: Run,
) -> Result<T, RegistryError>
where
    Run: FnOnce(MutationPhase) -> RunFuture,
    RunFuture: Future<Output = Result<T, RegistryError>>,
{
    let registered_operation_id = OperationId::new(operation_id.to_owned())?;
    let phase = MutationPhase::default();
    let workflow_phase = phase.clone();
    let result = service
        .run_mutation_workflow(registered_operation_id, phase, run(workflow_phase))
        .await;

    match result {
        Ok(result) => Ok(result),
        Err(error) => {
            if error.code == RegistryErrorCode::OutcomeUnknown {
                let _ = audit
                    .record_outcome_unknown_in(directory, connection_id, operation_id)
                    .await;
            } else if error.code != RegistryErrorCode::AuditIncomplete {
                let _ = audit
                    .record_failed_in(directory, connection_id, operation_id, error.code)
                    .await;
            }
            Err(error)
        }
    }
}
