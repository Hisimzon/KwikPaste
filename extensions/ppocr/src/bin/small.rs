fn main() -> anyhow::Result<()> {
    kwikpaste_ext_ppocr::run(kwikpaste_ext_ppocr::Models {
        det: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/small/det/inference.onnx"
        )),
        rec: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/small/rec/inference.onnx"
        )),
        det_config: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/small/det/inference.yml"
        )),
        rec_config: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/small/rec/inference.yml"
        )),
    })
}
