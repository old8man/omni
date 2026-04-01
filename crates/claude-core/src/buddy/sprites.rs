//! ASCII art sprites for companion species.
//!
//! Each sprite is 5 lines tall, 12 characters wide (after eye substitution).
//! Multiple animation frames per species for idle fidget animation.
//! Line 0 is the hat slot -- blank in frames 0-1; frame 2 may use it for
//! smoke, antenna, etc.

use super::types::*;

/// A sprite frame is 5 lines of ASCII art.
type Frame = [&'static str; 5];

/// Get the body frames for a species. Each species has 3 animation frames.
fn body_frames(species: Species) -> &'static [Frame] {
    match species {
        Species::Duck => &[
            [
                "            ",
                "    __      ",
                "  <({E} )___  ",
                "   (  ._>   ",
                "    `--\u{00B4}    ",
            ],
            [
                "            ",
                "    __      ",
                "  <({E} )___  ",
                "   (  ._>   ",
                "    `--\u{00B4}~   ",
            ],
            [
                "            ",
                "    __      ",
                "  <({E} )___  ",
                "   (  .__>  ",
                "    `--\u{00B4}    ",
            ],
        ],
        Species::Goose => &[
            [
                "            ",
                "     ({E}>    ",
                "     ||     ",
                "   _(__)_   ",
                "    ^^^^    ",
            ],
            [
                "            ",
                "    ({E}>     ",
                "     ||     ",
                "   _(__)_   ",
                "    ^^^^    ",
            ],
            [
                "            ",
                "     ({E}>>   ",
                "     ||     ",
                "   _(__)_   ",
                "    ^^^^    ",
            ],
        ],
        Species::Blob => &[
            [
                "            ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (      )  ",
                "   `----\u{00B4}   ",
            ],
            [
                "            ",
                "  .------.  ",
                " (  {E}  {E}  ) ",
                " (        ) ",
                "  `------\u{00B4}  ",
            ],
            [
                "            ",
                "    .--.    ",
                "   ({E}  {E})   ",
                "   (    )   ",
                "    `--\u{00B4}    ",
            ],
        ],
        Species::Cat => &[
            [
                "            ",
                "   /\\_/\\    ",
                "  ( {E}   {E})  ",
                "  (  \u{03C9}  )   ",
                "  (\")_(\")   ",
            ],
            [
                "            ",
                "   /\\_/\\    ",
                "  ( {E}   {E})  ",
                "  (  \u{03C9}  )   ",
                "  (\")_(\")~  ",
            ],
            [
                "            ",
                "   /\\-/\\    ",
                "  ( {E}   {E})  ",
                "  (  \u{03C9}  )   ",
                "  (\")_(\")   ",
            ],
        ],
        Species::Dragon => &[
            [
                "            ",
                "  /^\\  /^\\  ",
                " <  {E}  {E}  > ",
                " (   ~~   ) ",
                "  `-vvvv-\u{00B4}  ",
            ],
            [
                "            ",
                "  /^\\  /^\\  ",
                " <  {E}  {E}  > ",
                " (        ) ",
                "  `-vvvv-\u{00B4}  ",
            ],
            [
                "   ~    ~   ",
                "  /^\\  /^\\  ",
                " <  {E}  {E}  > ",
                " (   ~~   ) ",
                "  `-vvvv-\u{00B4}  ",
            ],
        ],
        Species::Octopus => &[
            [
                "            ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (______)  ",
                "  /\\/\\/\\/\\  ",
            ],
            [
                "            ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (______)  ",
                "  \\/\\/\\/\\/  ",
            ],
            [
                "     o      ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (______)  ",
                "  /\\/\\/\\/\\  ",
            ],
        ],
        Species::Owl => &[
            [
                "            ",
                "   /\\  /\\   ",
                "  (({E})({E}))  ",
                "  (  ><  )  ",
                "   `----\u{00B4}   ",
            ],
            [
                "            ",
                "   /\\  /\\   ",
                "  (({E})({E}))  ",
                "  (  ><  )  ",
                "   .----.   ",
            ],
            [
                "            ",
                "   /\\  /\\   ",
                "  (({E})(-))  ",
                "  (  ><  )  ",
                "   `----\u{00B4}   ",
            ],
        ],
        Species::Penguin => &[
            [
                "            ",
                "  .---.     ",
                "  ({E}>{E})     ",
                " /(   )\\    ",
                "  `---\u{00B4}     ",
            ],
            [
                "            ",
                "  .---.     ",
                "  ({E}>{E})     ",
                " |(   )|    ",
                "  `---\u{00B4}     ",
            ],
            [
                "  .---.     ",
                "  ({E}>{E})     ",
                " /(   )\\    ",
                "  `---\u{00B4}     ",
                "   ~ ~      ",
            ],
        ],
        Species::Turtle => &[
            [
                "            ",
                "   _,--._   ",
                "  ( {E}  {E} )  ",
                " /[______]\\ ",
                "  ``    ``  ",
            ],
            [
                "            ",
                "   _,--._   ",
                "  ( {E}  {E} )  ",
                " /[______]\\ ",
                "   ``  ``   ",
            ],
            [
                "            ",
                "   _,--._   ",
                "  ( {E}  {E} )  ",
                " /[======]\\ ",
                "  ``    ``  ",
            ],
        ],
        Species::Snail => &[
            [
                "            ",
                " {E}    .--.  ",
                "  \\  ( @ )  ",
                "   \\_`--\u{00B4}   ",
                "  ~~~~~~~   ",
            ],
            [
                "            ",
                "  {E}   .--.  ",
                "  |  ( @ )  ",
                "   \\_`--\u{00B4}   ",
                "  ~~~~~~~   ",
            ],
            [
                "            ",
                " {E}    .--.  ",
                "  \\  ( @  ) ",
                "   \\_`--\u{00B4}   ",
                "   ~~~~~~   ",
            ],
        ],
        Species::Ghost => &[
            [
                "            ",
                "   .----.   ",
                "  / {E}  {E} \\  ",
                "  |      |  ",
                "  ~`~``~`~  ",
            ],
            [
                "            ",
                "   .----.   ",
                "  / {E}  {E} \\  ",
                "  |      |  ",
                "  `~`~~`~`  ",
            ],
            [
                "    ~  ~    ",
                "   .----.   ",
                "  / {E}  {E} \\  ",
                "  |      |  ",
                "  ~~`~~`~~  ",
            ],
        ],
        Species::Axolotl => &[
            [
                "            ",
                "}~(______)~{",
                "}~({E} .. {E})~{",
                "  ( .--. )  ",
                "  (_/  \\_)  ",
            ],
            [
                "            ",
                "~}(______){~",
                "~}({E} .. {E}){~",
                "  ( .--. )  ",
                "  (_/  \\_)  ",
            ],
            [
                "            ",
                "}~(______)~{",
                "}~({E} .. {E})~{",
                "  (  --  )  ",
                "  ~_/  \\_~  ",
            ],
        ],
        Species::Capybara => &[
            [
                "            ",
                "  n______n  ",
                " ( {E}    {E} ) ",
                " (   oo   ) ",
                "  `------\u{00B4}  ",
            ],
            [
                "            ",
                "  n______n  ",
                " ( {E}    {E} ) ",
                " (   Oo   ) ",
                "  `------\u{00B4}  ",
            ],
            [
                "    ~  ~    ",
                "  u______n  ",
                " ( {E}    {E} ) ",
                " (   oo   ) ",
                "  `------\u{00B4}  ",
            ],
        ],
        Species::Cactus => &[
            [
                "            ",
                " n  ____  n ",
                " | |{E}  {E}| | ",
                " |_|    |_| ",
                "   |    |   ",
            ],
            [
                "            ",
                "    ____    ",
                " n |{E}  {E}| n ",
                " |_|    |_| ",
                "   |    |   ",
            ],
            [
                " n        n ",
                " |  ____  | ",
                " | |{E}  {E}| | ",
                " |_|    |_| ",
                "   |    |   ",
            ],
        ],
        Species::Robot => &[
            [
                "            ",
                "   .[||].   ",
                "  [ {E}  {E} ]  ",
                "  [ ==== ]  ",
                "  `------\u{00B4}  ",
            ],
            [
                "            ",
                "   .[||].   ",
                "  [ {E}  {E} ]  ",
                "  [ -==- ]  ",
                "  `------\u{00B4}  ",
            ],
            [
                "     *      ",
                "   .[||].   ",
                "  [ {E}  {E} ]  ",
                "  [ ==== ]  ",
                "  `------\u{00B4}  ",
            ],
        ],
        Species::Rabbit => &[
            [
                "            ",
                "   (\\__/)   ",
                "  ( {E}  {E} )  ",
                " =(  ..  )= ",
                "  (\")__(\" ) ",
            ],
            [
                "            ",
                "   (|__/)   ",
                "  ( {E}  {E} )  ",
                " =(  ..  )= ",
                "  (\")__(\" ) ",
            ],
            [
                "            ",
                "   (\\__/)   ",
                "  ( {E}  {E} )  ",
                " =( .  . )= ",
                "  (\")__(\" ) ",
            ],
        ],
        Species::Mushroom => &[
            [
                "            ",
                " .-o-OO-o-. ",
                "(__________)",
                "   |{E}  {E}|   ",
                "   |____|   ",
            ],
            [
                "            ",
                " .-O-oo-O-. ",
                "(__________)",
                "   |{E}  {E}|   ",
                "   |____|   ",
            ],
            [
                "   . o  .   ",
                " .-o-OO-o-. ",
                "(__________)",
                "   |{E}  {E}|   ",
                "   |____|   ",
            ],
        ],
        Species::Chonk => &[
            [
                "            ",
                "  /\\    /\\  ",
                " ( {E}    {E} ) ",
                " (   ..   ) ",
                "  `------\u{00B4}  ",
            ],
            [
                "            ",
                "  /\\    /|  ",
                " ( {E}    {E} ) ",
                " (   ..   ) ",
                "  `------\u{00B4}  ",
            ],
            [
                "            ",
                "  /\\    /\\  ",
                " ( {E}    {E} ) ",
                " (   ..   ) ",
                "  `------\u{00B4}~ ",
            ],
        ],
    }
}

/// Hat decoration lines (line 0 replacement when hat != none).
fn hat_line(hat: Hat) -> &'static str {
    match hat {
        Hat::None => "",
        Hat::Crown => "   \\^^^/    ",
        Hat::Tophat => "   [___]    ",
        Hat::Propeller => "    -+-     ",
        Hat::Halo => "   (   )    ",
        Hat::Wizard => "    /^\\     ",
        Hat::Beanie => "   (___)    ",
        Hat::Tinyduck => "    ,>      ",
    }
}

/// Render a sprite for the given companion bones and animation frame.
///
/// Returns a `Vec<String>` of lines. The hat slot (line 0) is replaced
/// with the hat decoration when applicable. If all frames have a blank
/// line 0 and no hat is present, the blank line is dropped to save
/// vertical space.
pub fn render_sprite(bones: &CompanionBones, frame: usize) -> Vec<String> {
    let frames = body_frames(bones.species);
    let body = &frames[frame % frames.len()];
    let eye_str = bones.eye.as_str();

    let mut lines: Vec<String> = body
        .iter()
        .map(|line| line.replace("{E}", eye_str))
        .collect();

    // Replace hat slot if line 0 is blank and hat is present
    if bones.hat != Hat::None && lines[0].trim().is_empty() {
        lines[0] = hat_line(bones.hat).to_string();
    }

    // Drop blank hat slot if all frames have blank line 0 (saves a row)
    if lines[0].trim().is_empty() && frames.iter().all(|f| f[0].trim().is_empty()) {
        lines.remove(0);
    }

    lines
}

/// Return the number of animation frames for a species.
pub fn sprite_frame_count(species: Species) -> usize {
    body_frames(species).len()
}

/// Render a compact inline face for the companion (for chat bubbles, etc.).
pub fn render_face(bones: &CompanionBones) -> String {
    let e = bones.eye.as_str();
    match bones.species {
        Species::Duck | Species::Goose => format!("({e}>"),
        Species::Blob => format!("({e}{e})"),
        Species::Cat => format!("={e}\u{03C9}{e}="),
        Species::Dragon => format!("<{e}~{e}>"),
        Species::Octopus => format!("~({e}{e})~"),
        Species::Owl => format!("({e})({e})"),
        Species::Penguin => format!("({e}>)"),
        Species::Turtle => format!("[{e}_{e}]"),
        Species::Snail => format!("{e}(@)"),
        Species::Ghost => format!("/{e}{e}\\"),
        Species::Axolotl => format!("}}{e}.{e}{{"),
        Species::Capybara => format!("({e}oo{e})"),
        Species::Cactus => format!("|{e}  {e}|"),
        Species::Robot => format!("[{e}{e}]"),
        Species::Rabbit => format!("({e}..{e})"),
        Species::Mushroom => format!("|{e}  {e}|"),
        Species::Chonk => format!("({e}.{e})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_bones(species: Species) -> CompanionBones {
        CompanionBones {
            rarity: Rarity::Common,
            species,
            eye: Eye::Dot,
            hat: Hat::None,
            shiny: false,
            stats: HashMap::new(),
        }
    }

    #[test]
    fn all_species_have_3_frames() {
        for &species in SPECIES {
            assert_eq!(
                sprite_frame_count(species),
                3,
                "{species} should have 3 frames"
            );
        }
    }

    #[test]
    fn render_sprite_basic() {
        let bones = test_bones(Species::Duck);
        let lines = render_sprite(&bones, 0);
        // Duck with no hat drops the blank hat line -> 4 lines
        assert_eq!(lines.len(), 4, "Duck sprite should be 4 lines (no hat)");
        // Should contain the eye character
        let joined = lines.join("\n");
        assert!(
            joined.contains("\u{00B7}"),
            "Duck sprite should contain dot eye"
        );
    }

    #[test]
    fn render_sprite_with_hat() {
        let mut bones = test_bones(Species::Duck);
        bones.hat = Hat::Crown;
        bones.rarity = Rarity::Rare;
        let lines = render_sprite(&bones, 0);
        // With hat, the hat line replaces the blank line 0
        assert!(
            lines[0].contains("^^^"),
            "Crown hat should show on line 0"
        );
    }

    #[test]
    fn render_face_all_species() {
        for &species in SPECIES {
            let bones = test_bones(species);
            let face = render_face(&bones);
            assert!(
                !face.is_empty(),
                "{species} face should not be empty"
            );
        }
    }

    #[test]
    fn sprite_lines_are_reasonable_width() {
        for &species in SPECIES {
            let bones = test_bones(species);
            for frame in 0..3 {
                let lines = render_sprite(&bones, frame);
                for (i, line) in lines.iter().enumerate() {
                    // Allow some slack for Unicode characters that are wider
                    assert!(
                        line.len() <= 20,
                        "{species} frame {frame} line {i} too wide: {} chars",
                        line.len()
                    );
                }
            }
        }
    }

    #[test]
    fn frame_cycling() {
        let bones = test_bones(Species::Blob);
        // Frame indices beyond count should wrap
        let f0 = render_sprite(&bones, 0);
        let f3 = render_sprite(&bones, 3);
        assert_eq!(f0, f3, "Frame 3 should wrap to frame 0");
    }
}
