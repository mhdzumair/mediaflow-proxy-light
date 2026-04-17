//! C FFI bridge for embedding MediaFlow Proxy in iOS apps.
//!
//! Feature-gated behind `ffi`. Exposes a C-compatible API that Swift can call
//! via a bridging header. Each `MediaflowServer` runs its own `tokio` runtime
//! on a dedicated background thread, completely isolated from the host process.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Log callback
// ---------------------------------------------------------------------------

type LogCallbackFn = unsafe extern "C" fn(*const c_char, *mut c_void);

struct LogCallbackState {
    cb: LogCallbackFn,
    userdata: *mut c_void,
}

// SAFETY: The caller is responsible for ensuring userdata lifetime and thread safety.
unsafe impl Send for LogCallbackState {}
unsafe impl Sync for LogCallbackState {}

static LOG_CALLBACK: OnceLock<Mutex<Option<LogCallbackState>>> = OnceLock::new();

fn log_callback_mutex() -> &'static Mutex<Option<LogCallbackState>> {
    LOG_CALLBACK.get_or_init(|| Mutex::new(None))
}

fn fire_log(line: &str) {
    if let Ok(guard) = log_callback_mutex().lock() {
        if let Some(ref state) = *guard {
            if let Ok(cstr) = CString::new(line) {
                unsafe { (state.cb)(cstr.as_ptr(), state.userdata) };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// C-compatible types
// ---------------------------------------------------------------------------

/// Status codes returned by FFI functions.
#[repr(C)]
pub enum MediaflowStatus {
    Ok = 0,
    ErrConfig = 1,
    ErrBind = 2,
    ErrAlreadyRunning = 3,
    ErrNotRunning = 4,
    ErrInternal = 5,
}

/// Configuration passed from Swift/ObjC into `mediaflow_server_create`.
/// All string pointers must remain valid until `mediaflow_server_start` returns.
#[repr(C)]
pub struct MediaflowConfig {
    /// Bind host, e.g. `"127.0.0.1"`. NULL → `"127.0.0.1"`.
    pub host: *const c_char,
    /// Bind port. 0 → let OS choose.
    pub port: u16,
    /// API password string. NULL or empty → no auth.
    pub api_password: *const c_char,
    /// Redis URL. NULL → in-memory cache only.
    pub redis_url: *const c_char,
    /// Reserved: JSON config override blob. NULL → ignored.
    pub config_json: *const c_char,
}

// ---------------------------------------------------------------------------
// Server handle
// ---------------------------------------------------------------------------

/// Opaque server handle returned to the caller.
pub struct MediaflowServer {
    host: String,
    port: u16,
    api_password: String,
    is_running: Arc<AtomicBool>,
    actual_port: Arc<AtomicU16>,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

// ---------------------------------------------------------------------------
// FFI functions
// ---------------------------------------------------------------------------

/// Create a new server handle.
///
/// Returns a heap-allocated `MediaflowServer` pointer, or NULL on failure.
/// The caller must eventually pass the pointer to `mediaflow_server_destroy`.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_server_create(
    cfg: *const MediaflowConfig,
) -> *mut MediaflowServer {
    if cfg.is_null() {
        fire_log("[ffi] mediaflow_server_create: null config");
        return std::ptr::null_mut();
    }

    let cfg = &*cfg;

    let host = if cfg.host.is_null() {
        "127.0.0.1".to_string()
    } else {
        match CStr::from_ptr(cfg.host).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                fire_log("[ffi] mediaflow_server_create: invalid host string");
                return std::ptr::null_mut();
            }
        }
    };

    let api_password = if cfg.api_password.is_null() {
        String::new()
    } else {
        CStr::from_ptr(cfg.api_password)
            .to_str()
            .unwrap_or("")
            .to_string()
    };

    let server = MediaflowServer {
        host,
        port: cfg.port,
        api_password,
        is_running: Arc::new(AtomicBool::new(false)),
        actual_port: Arc::new(AtomicU16::new(0)),
        shutdown_tx: Mutex::new(None),
    };

    Box::into_raw(Box::new(server))
}

/// Start the proxy server on a background thread.
///
/// Blocks until the server is bound and ready to accept connections.
/// Returns `MediaflowStatus::Ok` on success.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_server_start(srv: *mut MediaflowServer) -> MediaflowStatus {
    if srv.is_null() {
        return MediaflowStatus::ErrConfig;
    }
    let server = &*srv;

    if server.is_running.load(Ordering::SeqCst) {
        return MediaflowStatus::ErrAlreadyRunning;
    }

    let host = server.host.clone();
    let port = server.port;
    let api_password = server.api_password.clone();

    // Oneshot channel: background thread → this thread, signals that the
    // server is bound (or failed). The sender value is the actual bound port.
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<u16, String>>(1);
    // Shutdown channel.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let is_running = Arc::clone(&server.is_running);
    let actual_port = Arc::clone(&server.actual_port);

    // Spawn a dedicated OS thread with its own tokio runtime.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("tokio runtime: {e}")));
                return;
            }
        };

        rt.block_on(async move {
            // Build config from FFI params.
            let mut cfg = match crate::config::Config::from_env() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("config: {e}")));
                    return;
                }
            };

            cfg.server.host = host.clone();
            cfg.server.port = port;
            if !api_password.is_empty() {
                cfg.auth.api_password = api_password;
            }

            use crate::auth::middleware::AuthMiddleware;
            use crate::proxy::{handler, stream::StreamManager};
            use actix_cors::Cors;
            use actix_web::middleware::Logger;
            use actix_web::{middleware, web, App, HttpServer};
            use std::sync::Arc;

            let auth_middleware = AuthMiddleware::new(cfg.auth.api_password.clone());
            let stream_manager = StreamManager::new(cfg.proxy.clone());
            let server_config = Arc::new(cfg.clone());
            let host_bind = host.clone();

            let server = HttpServer::new(move || {
                let config = Arc::clone(&server_config);
                let cors = Cors::permissive();

                App::new()
                    .wrap(cors)
                    .wrap(Logger::new("%a - \"%r\" %s"))
                    .wrap(middleware::Compress::default())
                    .wrap(auth_middleware.clone())
                    .app_data(web::Data::new(stream_manager.clone()))
                    .app_data(web::Data::new(config.clone()))
                    .service(
                        web::scope("/proxy")
                            .route("/stream", web::get().to(handler::proxy_stream_get))
                            .route("/stream", web::head().to(handler::proxy_stream_head))
                            .route("/generate_url", web::post().to(handler::generate_url))
                            .route("/ip", web::get().to(handler::get_public_ip)),
                    )
                    .service(web::scope("/health").route("", web::get().to(|| async { "OK" })))
                    .default_service(web::route().to(|| async {
                        actix_web::HttpResponse::NotFound().json(serde_json::json!({
                            "error": "Not Found"
                        }))
                    }))
            });

            let server = match server.bind((host_bind.as_str(), port)) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("bind: {e}")));
                    return;
                }
            };

            // Determine the actual bound port (important when port=0).
            let bound_port = server.addrs().first().map(|a| a.port()).unwrap_or(port);

            // Signal readiness to the caller.
            let _ = ready_tx.send(Ok(bound_port));

            is_running.store(true, Ordering::SeqCst);
            actual_port.store(bound_port, Ordering::SeqCst);

            fire_log(&format!(
                "[ffi] Server listening on {host_bind}:{bound_port}"
            ));

            // Run until shutdown is requested.
            let srv = server.run();
            let handle = srv.handle();
            tokio::select! {
                _ = srv => {},
                _ = shutdown_rx => {
                    handle.stop(true).await;
                }
            }

            is_running.store(false, Ordering::SeqCst);
            fire_log("[ffi] Server stopped");
        });
    });

    // Wait for the background thread to signal readiness (or failure).
    match ready_rx.recv() {
        Ok(Ok(bound_port)) => {
            server.actual_port.store(bound_port, Ordering::SeqCst);
            if let Ok(mut guard) = server.shutdown_tx.lock() {
                *guard = Some(shutdown_tx);
            }
            MediaflowStatus::Ok
        }
        Ok(Err(msg)) => {
            fire_log(&format!("[ffi] Start failed: {msg}"));
            MediaflowStatus::ErrBind
        }
        Err(_) => {
            fire_log("[ffi] Start failed: channel closed");
            MediaflowStatus::ErrInternal
        }
    }
}

/// Stop the server gracefully.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_server_stop(srv: *mut MediaflowServer) -> MediaflowStatus {
    if srv.is_null() {
        return MediaflowStatus::ErrConfig;
    }
    let server = &*srv;

    if !server.is_running.load(Ordering::SeqCst) {
        return MediaflowStatus::ErrNotRunning;
    }

    if let Ok(mut guard) = server.shutdown_tx.lock() {
        if let Some(tx) = guard.take() {
            let _ = tx.send(());
            return MediaflowStatus::Ok;
        }
    }

    MediaflowStatus::ErrInternal
}

/// Destroy the server handle and free memory.
///
/// The server must be stopped before calling this, or resources may leak.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_server_destroy(srv: *mut MediaflowServer) {
    if !srv.is_null() {
        drop(Box::from_raw(srv));
    }
}

/// Returns `true` if the server is currently running.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_server_is_running(srv: *const MediaflowServer) -> bool {
    if srv.is_null() {
        return false;
    }
    (*srv).is_running.load(Ordering::SeqCst)
}

/// Returns the actual bound port (useful when `port` was 0 in config).
#[no_mangle]
pub unsafe extern "C" fn mediaflow_server_port(srv: *const MediaflowServer) -> u16 {
    if srv.is_null() {
        return 0;
    }
    (*srv).actual_port.load(Ordering::SeqCst)
}

/// Set a log callback. Pass NULL to clear.
///
/// The callback is invoked from background threads; it must be thread-safe.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_set_log_callback(
    cb: Option<LogCallbackFn>,
    userdata: *mut c_void,
) {
    if let Ok(mut guard) = log_callback_mutex().lock() {
        *guard = cb.map(|f| LogCallbackState { cb: f, userdata });
    }
}

/// Returns the base proxy URL as a heap-allocated C string.
///
/// Caller must pass the returned pointer to `mediaflow_free_string`.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_get_proxy_url(srv: *const MediaflowServer) -> *mut c_char {
    if srv.is_null() {
        return std::ptr::null_mut();
    }
    let server = &*srv;
    let port = server.actual_port.load(Ordering::SeqCst);
    let url = format!("http://{}:{}", server.host, port);
    match CString::new(url) {
        Ok(cs) => cs.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Free a C string previously returned by an FFI function.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

/// Persist a JSON config snapshot to the given file path.
///
/// Returns `MediaflowStatus::Ok` on success.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_save_config(
    srv: *const MediaflowServer,
    path: *const c_char,
) -> MediaflowStatus {
    if srv.is_null() || path.is_null() {
        return MediaflowStatus::ErrConfig;
    }
    let server = &*srv;
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return MediaflowStatus::ErrConfig,
    };

    let port = server.actual_port.load(Ordering::SeqCst);
    let json = serde_json::json!({
        "host": server.host,
        "port": port,
        "api_password": server.api_password,
    });

    match std::fs::write(path_str, json.to_string()) {
        Ok(_) => MediaflowStatus::Ok,
        Err(e) => {
            fire_log(&format!("[ffi] save_config failed: {e}"));
            MediaflowStatus::ErrInternal
        }
    }
}

/// Load a JSON config snapshot and apply it to the server handle.
///
/// Does NOT restart the server; only updates stored values.
#[no_mangle]
pub unsafe extern "C" fn mediaflow_load_config(
    srv: *mut MediaflowServer,
    path: *const c_char,
) -> MediaflowStatus {
    if srv.is_null() || path.is_null() {
        return MediaflowStatus::ErrConfig;
    }
    let server = &mut *srv;
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return MediaflowStatus::ErrConfig,
    };

    let data = match std::fs::read_to_string(path_str) {
        Ok(d) => d,
        Err(e) => {
            fire_log(&format!("[ffi] load_config read failed: {e}"));
            return MediaflowStatus::ErrInternal;
        }
    };

    let v: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            fire_log(&format!("[ffi] load_config parse failed: {e}"));
            return MediaflowStatus::ErrConfig;
        }
    };

    if let Some(h) = v["host"].as_str() {
        server.host = h.to_string();
    }
    if let Some(p) = v["port"].as_u64() {
        server.port = p as u16;
    }
    if let Some(pw) = v["api_password"].as_str() {
        server.api_password = pw.to_string();
    }

    MediaflowStatus::Ok
}
