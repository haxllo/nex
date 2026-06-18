fn main() {
    let svg_data = std::fs::read("../assets/nex.svg").expect("read svg");
    let rtree = usvg::Tree::from_data(&svg_data, &usvg::Options::default()).expect("parse svg");
    let w = rtree.size().width() as u32;
    let h = rtree.size().height() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).unwrap();
    resvg::render(&rtree, tiny_skia::Transform::default(), &mut pixmap.as_mut());

    // ICO max dimension is 256. Start with the SVG as a 256x256 version rendered
    // directly so we don't need to downscale.
    let mut small = tiny_skia::Pixmap::new(256, 256).unwrap();
    let scale = 256.0 / w as f32;
    let ts = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&rtree, ts, &mut small.as_mut());

    use image::codecs::ico::{IcoEncoder, IcoFrame};
    let frame =
        IcoFrame::as_png(small.data(), 256, 256, image::ExtendedColorType::Rgba8).expect("as_png");
    let mut ico = std::fs::File::create("nex.ico").expect("create ico");
    IcoEncoder::new(&mut ico)
        .encode_images(&[frame])
        .expect("encode ico");
}
