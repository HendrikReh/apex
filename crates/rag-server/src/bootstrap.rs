//! Startup banner and shutdown helpers for the `rag-server` binary.

/// Prints a green ASCII-art startup banner with version, license, and bind address info.
pub(crate) fn print_banner(version: &str, license_info: &str, bind_addr: &str) {
    const GREEN: &str = "\x1b[32m";
    const RESET: &str = "\x1b[0m";
    const INNER_WIDTH: usize = 84;

    let center_line = |text: &str| -> String {
        let width = text.chars().count();
        let padding = INNER_WIDTH.saturating_sub(width);
        let left = padding / 2;
        let right = padding - left;
        format!("  ║{}{}{}║", " ".repeat(left), text, " ".repeat(right))
    };

    let tagline = center_line(&format!("Apex Accelerator  ·  HTTP RAG API  ·  v{version}"));
    let license = center_line(license_info);
    let bind = center_line(&format!("bind {bind_addr}"));

    let banner = format!(
        r#"
  ╔════════════════════════════════════════════════════════════════════════════════════╗
  ║                                                                                    ║
  ║   █████╗ ██████╗ ███████╗██╗  ██╗                                                  ║
  ║  ██╔══██╗██╔══██╗██╔════╝╚██╗██╔╝                                                  ║
  ║  ███████║██████╔╝█████╗   ╚███╔╝                                                   ║
  ║  ██╔══██║██╔═══╝ ██╔══╝   ██╔██╗                                                   ║
  ║  ██║  ██║██║     ███████╗██╔╝ ██╗                                                  ║
  ║  ╚═╝  ╚═╝╚═╝     ╚══════╝╚═╝  ╚═╝                                                  ║
  ║                                                                                    ║
{tagline}
{license}
{bind}
  ║                                                                                    ║
  ╚════════════════════════════════════════════════════════════════════════════════════╝
"#,
        tagline = tagline,
        license = license,
        bind = bind,
    );

    println!("{GREEN}{banner}{RESET}");
}

/// Waits for Ctrl-C or SIGTERM and logs once a shutdown signal is observed.
pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
