use sid_core::adapters::sys::{ListeningPort, SysError};

pub(crate) fn list_listening_ports(
    _sys: &sysinfo::System,
) -> Result<Vec<ListeningPort>, SysError> {
    Err(SysError::Other("not yet implemented — Task 5".into()))
}
