use corten_auth::ZeroizedSecret;
use pam::{Client, PamReturnCode};
use tracing::{debug, error, info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    Success,
    InvalidCredentials,
    AccountLocked,
    ServiceUnavailable(String),
}

pub struct PamAuth;

impl PamAuth {
    /// Picks the single best installed PAM service from `/etc/pam.d/` so a wrong password
    /// only triggers ONE PAM attempt (preventing `pam_faillock` instant lockout and 10s delays).
    /// Also verifies that `/etc/pam.d/corten-lock` defines an `account` rule (required by
    /// `pam::Client::authenticate`'s `pam_acct_mgmt` check); otherwise falls back to `system-auth`.
    pub fn select_pam_service() -> &'static str {
        let candidates = ["corten-lock", "swaylock", "system-auth", "login", "passwd"];
        for service in candidates {
            let pam_file = format!("/etc/pam.d/{}", service);
            if std::path::Path::new(&pam_file).exists() {
                if service == "corten-lock"
                    && let Ok(contents) = std::fs::read_to_string(&pam_file)
                    && !contents.lines().any(|l| {
                        let t = l.trim_start();
                        !t.starts_with('#') && t.starts_with("account")
                    })
                    && std::path::Path::new("/etc/pam.d/system-auth").exists()
                {
                    return "system-auth";
                }
                return service;
            }
        }
        "login"
    }

    /// Runs synchronous PAM authentication on a dedicated blocking thread with a hard timeout
    /// so Tokio's async worker pool is never stalled and the lock screen can never hang forever.
    pub async fn authenticate_async(username: String, password: ZeroizedSecret) -> AuthOutcome {
        let handle =
            tokio::task::spawn_blocking(move || Self::authenticate(&username, password.as_str()));
        match tokio::time::timeout(std::time::Duration::from_secs(10), handle).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(err)) => {
                error!("PAM: spawn_blocking panicked or was cancelled: {:?}", err);
                AuthOutcome::ServiceUnavailable("Kimlik doğrulama görevi kesildi".into())
            }
            Err(_) => {
                error!("PAM: Authentication timed out after 10 seconds");
                AuthOutcome::ServiceUnavailable(
                    "PAM doğrulaması zaman aşımına uğradı, tekrar deneyin".into(),
                )
            }
        }
    }

    /// Authenticates a user against the system PAM stack (single attempt per submit).
    pub fn authenticate(username: &str, password: &str) -> AuthOutcome {
        let service = Self::select_pam_service();
        match Client::with_password(service) {
            Ok(mut client) => {
                client
                    .conversation_mut()
                    .set_credentials(username, password);
                match client.authenticate() {
                    Ok(_) => {
                        info!(
                            "PAM: Authentication successful for user '{}' via service '{}'",
                            username, service
                        );
                        AuthOutcome::Success
                    }
                    Err(err) => {
                        let code = err.0;
                        error!(
                            "PAM: Authentication failed for user '{}' via service '{}': {:?}",
                            username, service, err
                        );
                        match code {
                            PamReturnCode::MaxTries | PamReturnCode::Acct_Expired => {
                                AuthOutcome::AccountLocked
                            }
                            PamReturnCode::Service_Err
                            | PamReturnCode::System_Err
                            | PamReturnCode::Perm_Denied => AuthOutcome::ServiceUnavailable(
                                format!("PAM servis hatası ({service}: {code:?})"),
                            ),
                            _ => AuthOutcome::InvalidCredentials,
                        }
                    }
                }
            }
            Err(err) => {
                debug!("PAM: Service '{}' not available: {:?}", service, err);
                AuthOutcome::ServiceUnavailable(format!("PAM servisi '{service}' açılamadı"))
            }
        }
    }
}
