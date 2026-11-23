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
    pub fn select_pam_service() -> &'static str {
        let candidates = ["corten-lock", "swaylock", "system-auth", "login", "passwd"];
        for service in candidates {
            let pam_file = format!("/etc/pam.d/{}", service);
            if std::path::Path::new(&pam_file).exists() {
                return service;
            }
        }
        "login"
    }

    /// Runs synchronous PAM authentication on a dedicated blocking thread so Tokio's
    /// async worker pool is never stalled during `pam_faildelay`.
    pub async fn authenticate_async(username: String, password: ZeroizedSecret) -> AuthOutcome {
        tokio::task::spawn_blocking(move || Self::authenticate(&username, password.as_str()))
            .await
            .unwrap_or_else(|err| {
                error!("PAM: spawn_blocking panicked or was cancelled: {:?}", err);
                AuthOutcome::ServiceUnavailable("Kimlik doğrulama görevi kesildi".into())
            })
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
