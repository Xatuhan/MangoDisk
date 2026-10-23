use std::{marker::PhantomData, ptr, rc::Rc};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_NO_TOKEN, HANDLE},
    Security::{
        CreateRestrictedToken, ImpersonateLoggedOnUser, RevertToSelf, DISABLE_MAX_PRIVILEGE,
        TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY,
    },
    System::Threading::{
        GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, SetThreadToken,
    },
};

struct TokenHandle(HANDLE);
impl Drop for TokenHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Owns impersonation on the calling test thread, including any previous token.
/// It cannot move between threads; the original identity is restored on unwind.
pub(super) struct RestrictedToken {
    previous: Option<TokenHandle>,
    _restricted: TokenHandle,
    _thread_bound: PhantomData<Rc<()>>,
}

impl RestrictedToken {
    pub(super) fn enter() -> Self {
        unsafe {
            let mut previous = ptr::null_mut();
            if OpenThreadToken(
                GetCurrentThread(),
                TOKEN_QUERY | TOKEN_IMPERSONATE,
                1,
                &mut previous,
            ) == 0
            {
                assert_eq!(
                    GetLastError(),
                    ERROR_NO_TOKEN,
                    "inspect the test thread token"
                );
            }
            let previous = if previous.is_null() {
                None
            } else {
                Some(TokenHandle(previous))
            };
            let mut process = ptr::null_mut();
            assert_ne!(
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_DUPLICATE | TOKEN_QUERY,
                    &mut process
                ),
                0,
                "open the test process token"
            );
            let process = TokenHandle(process);
            let mut restricted = ptr::null_mut();
            assert_ne!(
                CreateRestrictedToken(
                    process.0,
                    DISABLE_MAX_PRIVILEGE,
                    0,
                    ptr::null(),
                    0,
                    ptr::null(),
                    0,
                    ptr::null(),
                    &mut restricted,
                ),
                0,
                "create the restricted test token"
            );
            let restricted = TokenHandle(restricted);
            assert_ne!(
                ImpersonateLoggedOnUser(restricted.0),
                0,
                "impersonate the restricted test token: {}",
                std::io::Error::last_os_error()
            );
            Self {
                previous,
                _restricted: restricted,
                _thread_bound: PhantomData,
            }
        }
    }
}

impl Drop for RestrictedToken {
    fn drop(&mut self) {
        unsafe {
            let restored = if let Some(previous) = &self.previous {
                SetThreadToken(ptr::null(), previous.0)
            } else {
                RevertToSelf()
            };
            assert_ne!(restored, 0, "restore the original test thread token");
        }
    }
}
