//! Recursos de empaquetado que tienen que ir dentro del ejecutable
//! (ver `native/docs/packaging.md`, sección "Windows").
//!
//! Solo actúa cuando el destino es Windows:
//! - incrusta el ícono y los metadatos de versión de `mergeon.exe` (lo que
//!   muestran el Explorador, la barra de tareas y "Propiedades > Detalles");
//! - en release marca el subsistema GUI para que la app no abra una consola.
//!
//! En Linux y macOS no hace nada: el ícono va en el `.desktop` / `Info.plist`.

use std::env;
use std::path::PathBuf;

/// Ícono multirresolución generado por `native/packaging/icons/make-icons.sh`.
const ICON: &str = "../../packaging/icons/mergeon.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let icon = manifest_dir.join(ICON);

    let mut resource = winresource::WindowsResource::new();
    // FileVersion / ProductVersion salen de CARGO_PKG_VERSION.
    resource
        .set("ProductName", "Mergeon")
        .set("FileDescription", "Mergeon")
        .set("CompanyName", "Spilveru")
        .set("LegalCopyright", "Copyright (c) Spilveru")
        .set("InternalName", "mergeon")
        .set("OriginalFilename", "mergeon.exe");
    if icon.is_file() {
        resource.set_icon(&icon.to_string_lossy());
    } else {
        println!(
            "cargo:warning=no se encontró {}: mergeon.exe saldrá sin ícono",
            icon.display()
        );
    }
    if let Err(error) = resource.compile() {
        // Sin rc.exe/windres el binario sigue sirviendo; solo pierde ícono y metadatos.
        println!("cargo:warning=no se pudieron incrustar los recursos de Windows: {error}");
    }

    // Equivale a `#![windows_subsystem = "windows"]` en release, sin tocar
    // `main.rs`: en debug se conserva la consola para ver los logs.
    let release = env::var("PROFILE").as_deref() == Ok("release");
    if release {
        match env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => {
                println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:WINDOWS");
                println!("cargo:rustc-link-arg-bins=/ENTRY:mainCRTStartup");
            }
            Ok("gnu") => println!("cargo:rustc-link-arg-bins=-mwindows"),
            _ => {}
        }
    }
}
