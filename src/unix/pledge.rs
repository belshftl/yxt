// SPDX-License-Identifier: MIT

#[derive(Debug, thiserror::Error)]
pub enum PledgeError {
    #[error("pledge is unsupported on this platform")]
    Unsupported,

    #[error("pledge promises/execpromises string had a NUL byte inside it")]
    InteriorNul,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(target_os = "openbsd")]
pub fn pledge(promises: &str, execpromises: Option<&str>) -> Result<(), PledgeError> {
    use std::ffi::CString;

    unsafe extern "C" {
        #[link_name = "pledge"]
        fn libc_pledge(promises: *const libc::c_char, execpromises: *const libc::c_char) -> libc::c_int;
    }

    let promises = CString::new(promises).map_err(|_| PledgeError::InteriorNul)?;
    let execpromises = execpromises.map(CString::new).transpose().map_err(|_| PledgeError::InteriorNul)?;
    if unsafe { libc_pledge(promises.as_ptr(), execpromises.as_ref().map_or(std::ptr::null(), |s| s.as_ptr())) } < 0 {
        Err(PledgeError::Io(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "openbsd"))]
pub fn pledge(_promises: &str, _execpromises: Option<&str>) -> Result<(), PledgeError> {
    Err(PledgeError::Unsupported)
}

pub fn try_pledge(promises: &str, execpromises: Option<&str>) -> Result<bool, PledgeError> {
    match pledge(promises, execpromises) {
        Ok(()) => Ok(true),
        Err(PledgeError::Unsupported) => Ok(false),
        Err(e) => Err(e),
    }
}
