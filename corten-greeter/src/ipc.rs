use greetd_ipc::codec::SyncCodec;
use greetd_ipc::{Request, Response};
use std::env;
use std::os::unix::net::UnixStream;
use tracing::{error, info};

pub struct GreetdClient;

impl GreetdClient {
    /// Attempts to log in and start a session using greetd IPC socket
    pub fn login_and_start_session(
        username: &str,
        password: &str,
        cmd: &[String],
    ) -> Result<(), String> {
        let sock_path = env::var("GREETD_SOCK").map_err(|_| {
            "GREETD_SOCK environment variable is not set (not running under greetd)".to_string()
        })?;

        let mut stream = UnixStream::connect(&sock_path).map_err(|e| {
            format!(
                "Failed to connect to greetd socket at '{}': {}",
                sock_path, e
            )
        })?;

        // 1. Create Session for user
        Request::CreateSession {
            username: username.to_string(),
        }
        .write_to(&mut stream)
        .map_err(|e| format!("Failed to send CreateSession: {:?}", e))?;

        let resp = Response::read_from(&mut stream)
            .map_err(|e| format!("Failed to read CreateSession response: {:?}", e))?;

        match resp {
            Response::AuthMessage { .. } => {
                // Respond to password prompt
                Request::PostAuthMessageResponse {
                    response: Some(password.to_string()),
                }
                .write_to(&mut stream)
                .map_err(|e| format!("Failed to send PostAuthMessageResponse: {:?}", e))?;

                let auth_resp = Response::read_from(&mut stream)
                    .map_err(|e| format!("Failed to read auth response: {:?}", e))?;

                match auth_resp {
                    Response::Success => {
                        info!("greetd: Authentication successful for user '{}'", username);
                    }
                    Response::Error { description, .. } => {
                        return Err(format!("Kimlik doğrulama başarısız: {}", description));
                    }
                    other => {
                        return Err(format!("Beklenmeyen yanıt: {:?}", other));
                    }
                }
            }
            Response::Success => {
                // User requires no authentication
                info!("greetd: User '{}' requires no password", username);
            }
            Response::Error { description, .. } => {
                return Err(format!("Kullanıcı başlatılamadı: {}", description));
            }
        }

        // 2. Start Session
        Request::StartSession {
            cmd: cmd.to_vec(),
            env: Vec::new(),
        }
        .write_to(&mut stream)
        .map_err(|e| format!("Failed to send StartSession: {:?}", e))?;

        let start_resp = Response::read_from(&mut stream)
            .map_err(|e| format!("Failed to read StartSession response: {:?}", e))?;

        match start_resp {
            Response::Success => {
                info!("greetd: Session started successfully");
                Ok(())
            }
            Response::Error { description, .. } => {
                error!("greetd: Failed to start session: {}", description);
                Err(format!("Oturum başlatılamadı: {}", description))
            }
            _ => Err("Bilinmeyen oturum başlatma hatası".into()),
        }
    }
}
