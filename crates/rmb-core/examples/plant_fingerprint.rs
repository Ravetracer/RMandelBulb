//! Prints a fingerprint of every preset plant's drawn segments (to check that changes to the
//! L-system code keep the classic presets identical).
fn main() {
    use rmb_core::plants::{PlantSpecies, PlantTemplate};
    for name in PlantSpecies::preset_names() {
        let t = PlantTemplate::grow(&PlantSpecies::preset(name).unwrap());
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for s in &t.segs {
            for v in s.a.iter().chain(&s.b).chain(&s.side).chain([&s.ra, &s.rb]) {
                h = (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        println!("{name}: {} segments, {h:016x}", t.segs.len());
    }
}
