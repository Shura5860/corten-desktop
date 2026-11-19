use pam::Client;
use tracing::{debug, error, info};

pub struct PamAuth;

impl PamAuth {
    /// Picks the single best installed PAM service from `/etc/pam.d/` so a wrong password
    /// only triggers ONE PAM attempt (preventing `pam_faillock` instant lockout and 10s delays).
    fn select_pam_service() -> &'static str {
        let candidates = ["corten-lock", "swaylock", "system-auth", "login", "passwd"];
        for service in candidates {
            let pam_file = format!("/etc/pam.d/{}", service);
            if std::path::Path::new(&pam_file).exists() {
                return service;
            }
        }
        "login"
    }

    /// Authenticates a user against the system PAM stack (single attempt per submit).
    pub fn authenticate(username: &str, password: &str) -> bool {
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
                        true
                    }
                    Err(err) => {
                        error!(
                            "PAM: Authentication failed for user '{}' via service '{}': {:?}",
                            username, service, err
                        );
                        false
                    }
                }
            }
            Err(err) => {
                debug!("PAM: Service '{}' not available: {:?}", service, err);
                false
            }
        }
    }
}
