use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/solyan-airplay-logo.png");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let png = include_bytes!("assets/solyan-airplay-logo.png");

    let source = image::load_from_memory(png)
        .expect("SolYan logo PNG must decode");

    // Windows Explorer/Desktop use different icon sizes depending on DPI,
    // view mode and shortcut cache. Embed a real multi-resolution ICO instead
    // of a single 256px frame.
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let rgba = source
            .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
        let icon_image = ico::IconImage::from_rgba_data(size, size, rgba.into_raw());
        icon_dir.add_entry(
            ico::IconDirEntry::encode(&icon_image)
                .expect("SolYan icon frame must encode"),
        );
    }

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
    res.set("ProductVersion", "0.2.14");
    res.set("FileVersion", "0.2.14");
    res.set("Comments", "Author: SolYan | https://www.youtube.com/@SolYan-Music");
    res.compile().expect("compile Windows resources");
}
