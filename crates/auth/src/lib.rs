//! Shared, backend-agnostic authentication state.
//!
//! The lock screen and greeter deliberately keep their authentication backends
//! separate (PAM and greetd respectively), while using this controller for the
//! sensitive credential lifecycle and UI state.

#[derive(Debug, Default, Clone)]
pub struct AuthController {
    username: String,
    secret: String,
    pending: bool,
    failed: bool,
}

impl AuthController {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            secret: String::new(),
            pending: false,
            failed: false,
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn set_username(&mut self, username: impl Into<String>) {
        self.username = username.into();
        self.failed = false;
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn set_secret(&mut self, secret: impl Into<String>) {
        self.secret = secret.into();
        self.failed = false;
    }

    /// Securely zeroizes secret memory using volatile writes before clearing buffer.
    pub fn clear_secret(&mut self) {
        unsafe {
            let bytes = self.secret.as_bytes_mut();
            for b in bytes {
                std::ptr::write_volatile(b, 0);
            }
        }
        self.secret.clear();
    }

    pub fn begin(&mut self) -> Option<(String, String)> {
        if self.pending || self.username.trim().is_empty() || self.secret.is_empty() {
            return None;
        }

        self.pending = true;
        Some((self.username.trim().to_string(), self.secret.clone()))
    }

    pub fn complete(&mut self, success: bool) {
        self.pending = false;
        self.failed = !success;
        // Credentials must not survive either a successful or failed attempt.
        self.clear_secret();
    }

    pub fn cancel(&mut self) {
        self.pending = false;
        self.failed = false;
        self.clear_secret();
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    pub fn has_failed(&self) -> bool {
        self.failed
    }
}

impl Drop for AuthController {
    fn drop(&mut self) {
        self.clear_secret();
    }
}

#[cfg(test)]
mod tests {
    use super::AuthController;

    #[test]
    fn begin_rejects_empty_credentials_and_duplicate_requests() {
        let mut auth = AuthController::new(" user ");
        assert!(auth.begin().is_none());
        auth.set_secret("secret");
        assert_eq!(auth.begin(), Some(("user".into(), "secret".into())));
        assert!(auth.begin().is_none());
    }

    #[test]
    fn completion_clears_secret_and_tracks_failure() {
        let mut auth = AuthController::new("user");
        auth.set_secret("secret");
        assert!(auth.begin().is_some());
        auth.complete(false);
        assert!(auth.secret().is_empty());
        assert!(auth.has_failed());
        assert!(!auth.is_pending());
    }

    #[test]
    fn successful_completion_clears_secret() {
        let mut auth = AuthController::new("user");
        auth.set_secret("secret");
        assert!(auth.begin().is_some());
        auth.complete(true);
        assert!(auth.secret().is_empty());
        assert!(!auth.has_failed());
    }
}
