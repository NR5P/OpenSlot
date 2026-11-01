//! Embeds the app icon (../assets/icon/openslot.ico) and version info in openslot.exe.

fn main() {
    println!("cargo::rerun-if-changed=openslot.rc");
    println!("cargo::rerun-if-changed=../assets/icon/openslot.ico");
    embed_resource::compile("openslot.rc", embed_resource::NONE).manifest_required().unwrap();
}
