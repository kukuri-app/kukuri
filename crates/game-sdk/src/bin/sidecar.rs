//! Windows の開発用 Sidecar（ADR 0070 §8.2）。Unity Editor の Play と Stop をまたいで profile の runtime を持ち続ける。
//!
//! `kukuri-sdk-sidecar run --profile <名前> --profile-dir <絶対 path>` で起動し、準備ができたら stderr に `ready` を書く。
//! `kukuri-sdk-sidecar stop --profile <名前> --profile-dir <絶対 path>` で止める（Ctrl+C でも止まる）。

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;

    use kukuri_game_sdk::{Mode, SdkClient, SdkConfig, sidecar};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let (Some(command), Some(profile), Some(profile_dir)) = (
        args.first().cloned(),
        value("--profile"),
        value("--profile-dir").map(std::path::PathBuf::from),
    ) else {
        eprintln!(
            "usage: kukuri-sdk-sidecar <run|stop> --profile <name> --profile-dir <absolute path>"
        );
        return ExitCode::from(2);
    };
    if profile.trim().is_empty() || !profile_dir.is_absolute() {
        eprintln!(
            "invalid_request: the profile name and an absolute profile directory are required"
        );
        return ExitCode::from(2);
    }
    match command.as_str() {
        "stop" => match sidecar::request_shutdown(profile, &profile_dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(code) => {
                eprintln!("{code}");
                ExitCode::from(3)
            }
        },
        "run" => {
            // panic の既定の出力は要求の中身を含みうるので、固定の文だけにする。
            std::panic::set_hook(Box::new(|_| eprintln!("kukuri sdk sidecar task panicked")));
            let mut client = match SdkClient::open(SdkConfig {
                profile,
                profile_dir: profile_dir.clone(),
                mode: Mode::Ffi,
            }) {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("internal_error: the runtime could not start");
                    return ExitCode::from(1);
                }
            };
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return ExitCode::from(1),
            };
            let (stop, stopped) = tokio::sync::watch::channel(false);
            let served = runtime.block_on(async {
                tokio::spawn(async move {
                    let _ = tokio::signal::ctrl_c().await;
                    let _ = stop.send(true);
                });
                sidecar::serve(&client, &profile_dir, stopped, || eprintln!("ready")).await
            });
            client.close();
            match served {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    // 同じ profile の Sidecar が既に動いている（同じ名前の pipe がある）など。
                    eprintln!("daemon_unavailable: {error}");
                    ExitCode::from(3)
                }
            }
        }
        _ => {
            eprintln!(
                "usage: kukuri-sdk-sidecar <run|stop> --profile <name> --profile-dir <absolute path>"
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("kukuri-sdk-sidecar is only available on Windows (ADR 0070 §8.2)");
    std::process::ExitCode::from(2)
}
