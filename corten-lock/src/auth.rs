use pam::Client;
use tracing::{debug, error, info};

pub struct PamAuth;

impl PamAuth {
    /// Authenticates a user against the system PAM stack
    pub fn authenticate(username: &str, password: &str) -> bool {
        let services = ["corten-lock", "swaylock", "system-auth", "login", "passwd"];
        for service in services {
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
                            return true;
                        }
                        Err(err) => {
                            error!(
                                "PAM: Authentication failed for user '{}' via service '{}': {:?}",
                                username, service, err
                            );
                        }
                    }
                }
                Err(err) => {
                    debug!(
                        "PAM: Service '{}' not available: {:?}",
                        service, err
                    );
                }
            }
        }
        false
    }
}
