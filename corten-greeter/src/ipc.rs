use std::env;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use corten_auth::ZeroizedSecret;
use greetd_ipc::codec::SyncCodec;
use greetd_ipc::{AuthMessageType, Request, Response};
use tracing::{error, info, warn};

const GREETD_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_AUTH_ROUNDS: usize = 8;

pub struct GreetdClient;

impl GreetdClient {
    /// Runs `greetd` authentication on a blocking thread with a hard timeout so Tokio's
    /// async worker pool is never stalled and the secret is zeroized on drop.
    pub async fn login_and_start_session_async(
        username: String,
        password: ZeroizedSecret,
        cmd: Vec<String>,
        is_wayland: bool,
    ) -> Result<(), String> {
        let handle = tokio::task::spawn_blocking(move || {
            Self::login_and_start_session(&username, password.as_str(), &cmd, is_wayland)
        });
        match tokio::time::timeout(Duration::from_secs(15), handle).await {
            Ok(Ok(res)) => res,
            Ok(Err(err)) => Err(format!("Oturum açma görevi kesildi: {err}")),
            Err(_) => Err("greetd yanıt vermedi (zaman aşımı). Tekrar deneyin.".into()),
        }
    }

    /// Attempts to log in and start a session using the `greetd` IPC socket.
    pub fn login_and_start_session(
        username: &str,
        password: &str,
        cmd: &[String],
        is_wayland: bool,
    ) -> Result<(), String> {
        let sock_path = env::var("GREETD_SOCK").map_err(|_| {
            "GREETD_SOCK ortam değişkeni tanımlı değil (greetd altında çalışmıyor).".to_string()
        })?;

        let mut stream = UnixStream::connect(&sock_path)
            .map_err(|e| format!("greetd soketine bağlanılamadı ('{}'): {}", sock_path, e))?;
        let _ = stream.set_read_timeout(Some(GREETD_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(GREETD_IO_TIMEOUT));

        // 1. Create session for user
        Request::CreateSession {
            username: username.to_string(),
        }
        .write_to(&mut stream)
        .map_err(|e| format!("CreateSession gönderilemedi: {e:?}"))?;

        let mut password_sent = false;
        for _ in 0..MAX_AUTH_ROUNDS {
            let resp = Response::read_from(&mut stream)
                .map_err(|e| format!("greetd yanıtı okunamadı: {e:?}"))?;

            match resp {
                Response::Success => {
                    info!("greetd: Authentication successful for user '{}'", username);
                    break;
                }
                Response::AuthMessage {
                    auth_message_type,
                    auth_message,
                } => {
                    let response_payload = match auth_message_type {
                        AuthMessageType::Secret => {
                            if password_sent {
                                Self::cancel_session(&mut stream);
                                return Err("Hatalı parola, lütfen tekrar deneyin.".into());
                            }
                            password_sent = true;
                            Some(password.to_string())
                        }
                        AuthMessageType::Visible => Some(username.to_string()),
                        AuthMessageType::Info | AuthMessageType::Error => {
                            warn!("greetd PAM message ({auth_message_type:?}): {auth_message}");
                            None
                        }
                    };

                    Request::PostAuthMessageResponse {
                        response: response_payload,
                    }
                    .write_to(&mut stream)
                    .map_err(|e| format!("Kimlik yanıtı gönderilemedi: {e:?}"))?;
                }
                Response::Error { description, .. } => {
                    Self::cancel_session(&mut stream);
                    return Err(format!("Kimlik doğrulama başarısız: {description}"));
                }
            }
        }

        // 2. Start Session with explicit session type environment
        let session_env = if is_wayland {
            vec!["XDG_SESSION_TYPE=wayland".to_string()]
        } else {
            vec!["XDG_SESSION_TYPE=x11".to_string()]
        };

        Request::StartSession {
            cmd: cmd.to_vec(),
            env: session_env,
        }
        .write_to(&mut stream)
        .map_err(|e| format!("StartSession gönderilemedi: {e:?}"))?;

        let start_resp = Response::read_from(&mut stream)
            .map_err(|e| format!("Oturum başlatma yanıtı okunamadı: {e:?}"))?;

        match start_resp {
            Response::Success => {
                info!("greetd: Session started successfully");
                Ok(())
            }
            Response::Error { description, .. } => {
                error!("greetd: Failed to start session: {}", description);
                Self::cancel_session(&mut stream);
                Err(format!("Oturum başlatılamadı: {description}"))
            }
            _ => {
                Self::cancel_session(&mut stream);
                Err("Bilinmeyen oturum başlatma hatası".into())
            }
        }
    }

    fn cancel_session(stream: &mut UnixStream) {
        let _ = Request::CancelSession.write_to(stream);
        let _ = Response::read_from(stream);
    }
}
