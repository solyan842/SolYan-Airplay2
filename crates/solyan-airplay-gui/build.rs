use base64::Engine as _;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/solyan-airplay-logo.0.b64");
    println!("cargo:rerun-if-changed=assets/solyan-airplay-logo.1.b64");
    println!("cargo:rerun-if-changed=assets/solyan-airplay-logo.2.b64");
    println!("cargo:rerun-if-changed=assets/solyan-airplay-logo.3.b64");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let b64 = concat!(
        include_str!("assets/solyan-airplay-logo.0.b64"),
        include_str!("assets/solyan-airplay-logo.1.b64"),
        include_str!("assets/solyan-airplay-logo.2.b64"),
        include_str!("assets/solyan-airplay-logo.3.b64"),
    );

    let png = base64::engine::general_purpose::STANDARD
        .decode(b64.split_whitespace().collect::<String>())
        .expect("SolYan logo base64 must decode");

    let image = image::load_from_memory(&png)
        .expect("SolYan logo PNG must decode")
        .resize_exact(256, 256, image::imageops::FilterType::Lanczos3)
        .to_rgba8();

    let icon_image = ico::IconImage::from_rgba_data(256, 256, image.into_raw());
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    icon_dir.add_entry(
        ico::IconDirEntry::encode(&icon_image)
            .expect("SolYan icon must encode"),
    );

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico_path = out_dir.join("solyan-airplay.ico");
    let file = File::create(&ico_path).expect("create generated .ico");
    icon_dir
        .write(BufWriter::new(file))
        .expect("write generated .ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().expect("icon path UTF-8"));
    res.set("ProductName", "SolYan AirPlay2");
    res.set("FileDescription", "SolYan AirPlay2 - Windows AirPlay sender");
    res.set("CompanyName", "SolYan");
    res.set("LegalCopyright", "© 2026 SolYan");
    res.set("ProductVersion", "0.2.0");
    res.set("FileVersion", "0.2.0");
    res.set("Comments", "Author: SolYan | https://www.youtube.com/@SolYan-Music");
    res.compile().expect("compile Windows resources");
}
