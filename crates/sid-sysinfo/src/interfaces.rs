use sid_core::adapters::sys::{NetInterface, SysError};

pub(crate) fn list_interfaces(_sys: &mut sysinfo::System) -> Result<Vec<NetInterface>, SysError> {
    Err(SysError::Other("not yet implemented — Task 6".into()))
}
