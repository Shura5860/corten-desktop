//! Shared, backend-agnostic authentication state.
//!
//! The lock screen and greeter deliberately keep their authentication backends
//! separate (PAM and greetd respectively), while using this controller for the
//! sensitive credential lifecycle and UI state.

use std::ops::Deref;
use std::sync::atomic::{compiler_fence, Ordering};

/// Overwrites all bytes of `buf` (including spare capacity up to `len`) with zeros
/// using volatile writes and a compiler fence so the optimizer cannot elide erasure.
fn zeroize_string(buf: &mut String) {
    // SAFETY: We overwrite initialized bytes with 0 (valid UTF-8 NUL) and immediately clear.
    unsafe {
        let bytes = buf.as_bytes_mut();
        for b in bytes {
            std::ptr::write_volatile(b, 0);
        }
    }
    compiler_fence(Ordering::SeqCst);
    buf.clear();
}

/// A heap-allocated secret string that securely zeroizes its memory on [`Drop`].
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ZeroizedSecret(String);

impl ZeroizedSecret {
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for ZeroizedSecret {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Debug for ZeroizedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ZeroizedSecret([REDACTED])")
    }
}

impl Drop for ZeroizedSecret {
    fn drop(&mut self) {
        zeroize_string(&mut self.0);
    }
}

#[derive(Debug, Default, Clone)]
pub struct AuthController {
    username: String,
    secret: String,
    pending: bool,
    failed: bool,
    failed_attempts: u32,
}

impl AuthController {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            secret: String::new(),
            pending: false,
            failed: false,
            failed_attempts: 0,
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

    /// Returns the number of Unicode scalar values in the current secret (UTF-8 safe).
    pub fn char_count(&self) -> usize {
        self.secret.chars().count()
    }

    /// Replaces the secret after securely zeroizing the previous buffer.
    pub fn set_secret(&mut self, secret: impl Into<String>) {
        if self.pending {
            return;
        }
        zeroize_string(&mut self.secret);
        self.secret = secret.into();
        self.failed = false;
    }

    /// Appends a single character in-place without cloning the secret onto the heap.
    /// Returns `false` if authentication is currently pending.
    pub fn push_char(&mut self, ch: char) -> bool {
        if self.pending || ch.is_control() {
            return false;
        }
        self.secret.push(ch);
        self.failed = false;
        true
    }

    /// Removes the last Unicode character in-place and zeroizes the vacated tail bytes.
    /// Returns `false` if authentication is currently pending or the buffer is empty.
    pub fn pop_char(&mut self) -> bool {
        if self.pending {
            return false;
        }
        let Some(ch) = self.secret.pop() else {
            return false;
        };
        let new_len = self.secret.len();
        let removed_len = ch.len_utf8();
        // Zeroize the spare bytes where the popped UTF-8 character lived.
        let vec = unsafe { self.secret.as_mut_vec() };
        if vec.capacity() >= new_len + removed_len {
            let ptr = vec.as_mut_ptr();
            for offset in new_len..(new_len + removed_len) {
                unsafe {
                    std::ptr::write_volatile(ptr.add(offset), 0);
                }
            }
            compiler_fence(Ordering::SeqCst);
        }
        self.failed = false;
        true
    }

    /// Deletes the trailing word (Ctrl+Backspace), zeroizing every removed character.
    pub fn pop_word(&mut self) -> bool {
        if self.pending || self.secret.is_empty() {
            return false;
        }
        let mut changed = false;
        while self.secret.chars().last().is_some_and(char::is_whitespace) {
            changed |= self.pop_char();
        }
        while self.secret.chars().last().is_some_and(|c| !c.is_whitespace()) {
            changed |= self.pop_char();
        }
        changed
    }

    /// Securely zeroizes secret memory using volatile writes before clearing buffer.
    pub fn clear_secret(&mut self) {
        zeroize_string(&mut self.secret);
    }

    pub fn begin(&mut self) -> Option<(String, String)> {
        if self.pending || self.username.trim().is_empty() || self.secret.is_empty() {
            return None;
        }

        self.pending = true;
        Some((self.username.trim().to_string(), self.secret.clone()))
    }

    /// Starts an authentication attempt, returning a [`ZeroizedSecret`] that wipes
    /// itself automatically when dropped.
    pub fn begin_zeroized(&mut self) -> Option<(String, ZeroizedSecret)> {
        if self.pending || self.username.trim().is_empty() || self.secret.is_empty() {
            return None;
        }

        self.pending = true;
        Some((
            self.username.trim().to_string(),
            ZeroizedSecret::new(self.secret.clone()),
        ))
    }

    pub fn complete(&mut self, success: bool) {
        self.pending = false;
        self.failed = !success;
        if success {
            self.failed_attempts = 0;
        } else {
            self.failed_attempts = self.failed_attempts.saturating_add(1);
        }
        // Credentials must not survive either a successful or failed attempt.
        self.clear_secret();
    }

    pub fn cancel(&mut self) {
        self.pending = false;
        self.failed = false;
        self.clear_secret();
    }

    /// Clears the current input buffer and error flag only when no authentication
    /// request is in flight (preventing concurrent PAM races on Escape).
    pub fn cancel_input(&mut self) {
        if self.pending {
            return;
        }
        self.failed = false;
        self.clear_secret();
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    pub fn has_failed(&self) -> bool {
        self.failed
    }

    pub fn failed_attempts(&self) -> u32 {
        self.failed_attempts
    }
}

impl Drop for AuthController {
    fn drop(&mut self) {
        self.clear_secret();
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthController, ZeroizedSecret};

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
        assert_eq!(auth.failed_attempts(), 1);
        assert!(!auth.is_pending());
    }

    #[test]
    fn successful_completion_clears_secret_and_resets_attempts() {
        let mut auth = AuthController::new("user");
        auth.set_secret("wrong");
        let _ = auth.begin();
        auth.complete(false);
        assert_eq!(auth.failed_attempts(), 1);

        auth.set_secret("secret");
        assert!(auth.begin().is_some());
        auth.complete(true);
        assert!(auth.secret().is_empty());
        assert!(!auth.has_failed());
        assert_eq!(auth.failed_attempts(), 0);
        assert!(!auth.is_pending());
    }

    #[test]
    fn pending_blocks_mutations_and_cancel_input() {
        let mut auth = AuthController::new("user");
        assert!(auth.push_char('ş'));
        assert!(auth.push_char('i'));
        assert_eq!(auth.char_count(), 2);

        let creds = auth.begin_zeroized();
        assert!(creds.is_some());
        assert!(!auth.push_char('x'));
        assert!(!auth.pop_char());
        assert!(!auth.pop_word());
        auth.cancel_input();
        assert!(auth.is_pending());

        auth.complete(false);
        assert!(!auth.is_pending());
    }

    #[test]
    fn pop_word_and_zeroized_secret_debug_redacted() {
        let mut auth = AuthController::new("user");
        for c in "merhaba dünya".chars() {
            assert!(auth.push_char(c));
        }
        assert_eq!(auth.char_count(), 13);
        assert!(auth.pop_word());
        assert_eq!(auth.secret(), "merhaba ");
        assert!(auth.pop_word());
        assert_eq!(auth.secret(), "");

        let secret = ZeroizedSecret::new("top-secret".into());
        assert_eq!(format!("{secret:?}"), "ZeroizedSecret([REDACTED])");
    }
}
