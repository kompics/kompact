use rustc_version::{version_meta, Channel};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(nightly)");
    let version_meta = version_meta().unwrap();
    if version_meta.channel == Channel::Nightly {
        println!("cargo::rustc-cfg=nightly");
    }
}
