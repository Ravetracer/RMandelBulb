//! Prints the machine code of a Mandelbulb3D formula: `m3f_disasm <file.m3f>`.
use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter};
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let text = String::from_utf8_lossy(&std::fs::read(&path).expect("read")).into_owned();
    let name = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy().into_owned();
    let m = rmb_core::formula::m3f::M3f::parse(&name, &text).expect("parse");
    println!("DE option {}, options: {:?}", m.de_option, m.options.iter().map(|o| (&o.name, o.kind)).collect::<Vec<_>>());
    let mut d = Decoder::with_ip(32, &m.code, 0, DecoderOptions::NONE);
    let mut f = IntelFormatter::new();
    let mut ins = Instruction::default();
    let mut out = String::new();
    while d.can_decode() {
        d.decode_out(&mut ins);
        out.clear();
        f.format(&ins, &mut out);
        println!("{:04x}: {out}", ins.ip());
    }
}
