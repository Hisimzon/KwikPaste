fn main() {
    let options = wx_ime_sdk::ClientOptions::new(".", "KwikPaste");
    let _ = (options, kwikpaste_ext_protocol::OCR_PROTOCOL);
}
