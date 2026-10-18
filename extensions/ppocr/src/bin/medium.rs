fn main() -> anyhow::Result<()> {
    kwikpaste_ext_ppocr::run(kwikpaste_ext_ppocr::Models {
        det: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/medium/det/inference.onnx"
        )),
        rec: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/medium/rec/inference.onnx"
        )),
        det_config: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/medium/det/inference.yml"
        )),
        rec_config: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/.cache/models/medium/rec/inference.yml"
        )),
    })
}
