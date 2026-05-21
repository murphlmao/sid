use sid_core::adapters::sys::{ProcessInfo, SysError};

pub(crate) fn list_processes(_sys: &mut sysinfo::System) -> Result<Vec<ProcessInfo>, SysError> {
    Err(SysError::Other("not yet implemented — Task 4".into()))
}
