use usvg::SystemFontDB;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        println!("Usage:\n\tminimal <in-svg> <out-png>");
        return;
    }

    let mut opt = usvg::Options::default();
    opt.path = Some(args[1].clone().into());
    opt.fontdb.load_system_fonts();

    let rtree = usvg::Tree::from_file(&args[1], &opt).unwrap();
    let img = resvg_skia::render_to_image(&rtree, usvg::FitTo::Original, None).unwrap();
    img.save_png(&args[2]);
}
