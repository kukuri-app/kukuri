//! GPUのない環境では、ソフトウェア描画をDMA-BUF経由で転送しない。

use std::{ffi::OsStr, path::Path};

fn renderer_override(devices: &Path, explicit: Option<&OsStr>) -> Option<&'static str> {
    (explicit.is_none()
        && !["dxg", "nvidiactl"]
            .iter()
            .any(|name| devices.join(name).exists())
        && !std::fs::read_dir(devices.join("dri"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.file_name().as_encoded_bytes().starts_with(b"renderD")))
    .then_some("1")
}

/// # Safety
/// GTK等の初期化・スレッド生成より前のmainからだけ呼ぶこと。
pub(crate) unsafe fn configure() {
    let explicit = std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER")
        .or_else(|| std::env::var_os("WEBKIT_DMABUF_RENDERER_FORCE_SHM"));
    if let Some(value) = renderer_override(Path::new("/dev"), explicit.as_deref()) {
        // SAFETY: 呼出元はスレッドを生成する前のmainに限定する。
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", value) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_transport_preserves_explicit_choices_and_gpu_paths() {
        let devices = tempfile::tempdir().unwrap();
        assert_eq!(renderer_override(devices.path(), None), Some("1"));
        for value in ["0", "1", ""] {
            assert_eq!(
                renderer_override(devices.path(), Some(OsStr::new(value))),
                None
            );
        }
        std::fs::create_dir(devices.path().join("dri")).unwrap();
        std::fs::write(devices.path().join("dri/card0"), []).unwrap();
        assert_eq!(renderer_override(devices.path(), None), Some("1"));
        for name in ["dri/renderD128", "dxg", "nvidiactl"] {
            let device = devices.path().join(name);
            std::fs::write(&device, []).unwrap();
            assert_eq!(renderer_override(devices.path(), None), None);
            std::fs::remove_file(device).unwrap();
        }
    }
}
