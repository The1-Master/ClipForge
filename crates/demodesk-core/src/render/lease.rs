//! Cross-process serialization shared with the local AI and Steam GC worker.
use anyhow::{anyhow, Result};

#[cfg(windows)]
pub struct ComputeLease(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ComputeLease {
    pub fn try_acquire() -> Result<Option<Self>> {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        let name: Vec<u16> = "Local\\ClipForgeCompute\0".encode_utf16().collect();
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            match WaitForSingleObject(handle, 0) {
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self(handle))),
                WAIT_TIMEOUT => {
                    CloseHandle(handle);
                    Ok(None)
                }
                _ => {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(handle);
                    Err(error.into())
                }
            }
        }
    }
}

#[cfg(windows)]
impl Drop for ComputeLease {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
pub struct ComputeLease;
#[cfg(not(windows))]
impl ComputeLease {
    pub fn try_acquire() -> Result<Option<Self>> {
        Err(anyhow!("Recording compute lease requires Windows"))
    }
}

pub(super) fn acquire(cancel: &std::sync::atomic::AtomicBool) -> Result<ComputeLease> {
    acquire_with_wait(cancel, || {})
}

pub(super) fn acquire_with_wait(
    cancel: &std::sync::atomic::AtomicBool,
    mut waiting: impl FnMut(),
) -> Result<ComputeLease> {
    use std::sync::atomic::Ordering;
    let mut reported = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        if let Some(lease) = ComputeLease::try_acquire()? {
            return Ok(lease);
        }
        if !reported {
            waiting();
            reported = true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
