use sid_core::adapters::sys::{Pid, Signal, SysError};

pub(crate) fn kill_process(_pid: Pid, _sig: Signal) -> Result<(), SysError> {
    Err(SysError::Other("not yet implemented — Task 7".into()))
}
