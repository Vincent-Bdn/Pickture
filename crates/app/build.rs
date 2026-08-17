//! Embeds the Windows executable resources.
//!
//! The runtime window icon is rasterised from the mark in `main.rs`, which
//! covers the taskbar while Pickture is running. This covers the other half:
//! the icon Explorer shows for `pickture.exe` itself, which comes from a
//! resource compiled into the binary and cannot be set at runtime.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");

    #[cfg(windows)]
    {
        let icon = std::path::Path::new("../../assets/icon.ico");
        if !icon.exists() {
            println!("cargo:warning=assets/icon.ico missing; exe will use the default icon");
            return;
        }
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("ProductName", "Pickture");
        res.set("FileDescription", "Pickture — photo culling");
        res.set("LegalCopyright", "PolyForm Noncommercial License 1.0.0");
        // A missing resource compiler must not break the build for everyone
        // else; the binary is perfectly usable without its Explorer icon.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed exe resources: {e}");
        }
    }
}
