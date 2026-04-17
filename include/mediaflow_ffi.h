/**
 * mediaflow_ffi.h — C interface for embedding MediaFlow Proxy Light in iOS apps.
 *
 * Build the Rust library with the `ffi` feature flag:
 *   cargo build --target aarch64-apple-ios --features ffi,...
 *
 * Swift usage:
 *   - Add this header to your bridging header:
 *       #import "mediaflow_ffi.h"
 *   - Wrap via RustServer.swift (see ios/ project)
 */

#ifndef MEDIAFLOW_FFI_H
#define MEDIAFLOW_FFI_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---------------------------------------------------------------------------
 * Opaque handle to a running server instance.
 * Allocate with mediaflow_server_create(); free with mediaflow_server_destroy().
 * ------------------------------------------------------------------------- */
typedef struct MediaflowServer MediaflowServer;

/* ---------------------------------------------------------------------------
 * Server configuration passed at creation time.
 * All string pointers must remain valid until mediaflow_server_create() returns.
 * ------------------------------------------------------------------------- */
typedef struct {
    /** Bind address, e.g. "0.0.0.0" or "127.0.0.1". */
    const char *host;
    /** TCP port to listen on (e.g. 8888). */
    uint16_t port;
    /** API password. Pass NULL or "" to disable auth. */
    const char *api_password;
    /** Redis URL, e.g. "redis://localhost:6379". Pass NULL or "" to disable. */
    const char *redis_url;
    /** JSON blob for additional config (transport_routes, etc.).
     *  Pass NULL to use defaults. */
    const char *config_json;
} MediaflowConfig;

/* ---------------------------------------------------------------------------
 * Status codes returned by lifecycle functions.
 * ------------------------------------------------------------------------- */
typedef enum {
    MEDIAFLOW_OK = 0,
    MEDIAFLOW_ERR_CONFIG = 1,
    MEDIAFLOW_ERR_BIND = 2,
    MEDIAFLOW_ERR_ALREADY_RUNNING = 3,
    MEDIAFLOW_ERR_NOT_RUNNING = 4,
    MEDIAFLOW_ERR_INTERNAL = 5,
} MediaflowStatus;

/* ---------------------------------------------------------------------------
 * Log callback type.
 * Called from the server's background thread; must be thread-safe.
 * `line`     — null-terminated log line (UTF-8).
 * `userdata` — opaque pointer passed to mediaflow_set_log_callback().
 * ------------------------------------------------------------------------- */
typedef void (*MediaflowLogCallback)(const char *line, void *userdata);

/* ---------------------------------------------------------------------------
 * Server lifecycle
 * ------------------------------------------------------------------------- */

/**
 * Create a new server handle with the given configuration.
 * Returns NULL on allocation failure. Does NOT start the server yet.
 */
MediaflowServer *mediaflow_server_create(const MediaflowConfig *config);

/**
 * Start the server (binds the port, spawns the Tokio runtime).
 * Non-blocking: returns once the server is listening, or on error.
 */
MediaflowStatus mediaflow_server_start(MediaflowServer *server);

/**
 * Gracefully stop the server and release the Tokio runtime.
 */
MediaflowStatus mediaflow_server_stop(MediaflowServer *server);

/**
 * Free the server handle and all associated resources.
 * The server must be stopped before calling this (or stop is called automatically).
 */
void mediaflow_server_destroy(MediaflowServer *server);

/* ---------------------------------------------------------------------------
 * Status queries
 * ------------------------------------------------------------------------- */

/** Returns true if the server is currently accepting connections. */
bool mediaflow_server_is_running(const MediaflowServer *server);

/** Returns the actual port the server is bound to (useful if port 0 was used). */
uint16_t mediaflow_server_port(const MediaflowServer *server);

/* ---------------------------------------------------------------------------
 * Logging
 * ------------------------------------------------------------------------- */

/**
 * Register a callback to receive log lines from the server.
 * Pass NULL to unregister. `userdata` is forwarded to every callback invocation.
 * Thread-safe: may be called before or after mediaflow_server_start().
 */
void mediaflow_set_log_callback(MediaflowLogCallback callback, void *userdata);

/* ---------------------------------------------------------------------------
 * Helpers
 * ------------------------------------------------------------------------- */

/**
 * Return the base proxy URL as a heap-allocated C string, e.g. "http://127.0.0.1:8888".
 * The caller must free the returned string with mediaflow_free_string().
 * Returns NULL if the server is not running.
 */
char *mediaflow_get_proxy_url(const MediaflowServer *server);

/**
 * Free a C string returned by any mediaflow_* function that returns char*.
 */
void mediaflow_free_string(char *s);

/**
 * Persist the current configuration to a file at `path` (TOML format).
 * Returns MEDIAFLOW_OK on success.
 */
MediaflowStatus mediaflow_save_config(const MediaflowServer *server, const char *path);

/**
 * Load configuration from a TOML file at `path` into a stopped server.
 * Returns MEDIAFLOW_ERR_ALREADY_RUNNING if the server is running.
 */
MediaflowStatus mediaflow_load_config(MediaflowServer *server, const char *path);

#ifdef __cplusplus
}
#endif

#endif /* MEDIAFLOW_FFI_H */
