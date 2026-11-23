//! The port's pure model: the two config files, what a dis-enchant does to a stack's enchantments,
//! the experience arithmetic and the table's particles. Nothing here touches the game.

/// The server file's text when it does not exist yet (upstream's `disenchanting_table-server.toml`).
pub const DEFAULT_SERVER: &str = include_str!("server.toml");
/// The client file's text when it does not exist yet (upstream's `disenchanting_table-client.toml`).
pub const DEFAULT_CLIENT: &str = include_str!("client.toml");

/// `DisEnchantingTableServer`'s settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerSettings {
    pub automatic_disenchanting: bool,
    pub automatic_disenchanting_ticks: i32,
    pub resets_repair_cost: bool,
    pub requires_experience: bool,
    pub uses_points: bool,
    pub experience_cost: i32,
}

impl Default for ServerSettings {
    /// Upstream's field initialisers, the same values as the default file.
    fn default() -> Self {
        Self {
            automatic_disenchanting: false,
            automatic_disenchanting_ticks: 10,
            resets_repair_cost: true,
            requires_experience: true,
            uses_points: true,
            experience_cost: 25,
        }
    }
}

/// `DisEnchantingTableClient`'s settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientSettings {
    pub render_block_particles: bool,
    pub render_experience_cost: bool,
    pub render_table_item: bool,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            render_block_particles: true,
            render_experience_cost: true,
            render_table_item: true,
        }
    }
}

/// The `key = value` pairs of a flat TOML file, comments and blank lines left out. A line that is
/// not a pair is skipped, as is a value of the wrong type in the readers below: the setting keeps
/// its default.
fn pairs(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|line| {
        let line = line.split('#').next().unwrap_or("");
        let (key, value) = line.split_once('=')?;
        Some((key.trim(), value.trim()))
    })
}

fn boolean(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// A TOML integer that fits an `i32` (upstream's `Math.toIntExact`), `_` separators allowed.
fn int(value: &str) -> Option<i32> {
    value.replace('_', "").parse().ok()
}

impl ServerSettings {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for (key, value) in pairs(text) {
            match key {
                "automatic_disenchanting" => set(&mut s.automatic_disenchanting, boolean(value)),
                "automatic_disenchanting_ticks" => {
                    set(&mut s.automatic_disenchanting_ticks, int(value))
                }
                "resets_repair_cost" => set(&mut s.resets_repair_cost, boolean(value)),
                "requires_experience" => set(&mut s.requires_experience, boolean(value)),
                "uses_points" => set(&mut s.uses_points, boolean(value)),
                "experience_cost" => set(&mut s.experience_cost, int(value)),
                _ => {}
            }
        }
        s
    }

    /// The experience a dis-enchant costs, as one signed number: 0 when it costs nothing, the
    /// points when positive, the levels when negative.
    pub fn rule(&self) -> i64 {
        if !self.requires_experience || self.experience_cost <= 0 {
            return 0;
        }
        let cost = i64::from(self.experience_cost);
        if self.uses_points { cost } else { -cost }
    }

    /// [`Self::rule`] as the menu's data slot 1 sends it to the screen. A data slot holds 16 bits,
    /// so a larger cost is clamped there, which only changes what the screen predicts: the server
    /// checks the real cost.
    pub fn cost_rule(&self) -> i32 {
        self.rule().clamp(-i64::from(i16::MAX), i64::from(i16::MAX)) as i32
    }
}

impl ClientSettings {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for (key, value) in pairs(text) {
            match key {
                "render_block_particles" => set(&mut s.render_block_particles, boolean(value)),
                "render_experience_cost" => set(&mut s.render_experience_cost, boolean(value)),
                "render_table_item" => set(&mut s.render_table_item, boolean(value)),
                _ => {}
            }
        }
        s
    }
}

fn set<T>(field: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *field = value;
    }
}

// ---- experience -------------------------------------------------------------------------------

/// `Player.getXpNeededForNextLevel` at `level`.
pub fn xp_needed_for_next_level(level: i32) -> i64 {
    let level = i64::from(level);
    if level >= 30 {
        112 + (level - 30) * 9
    } else if level >= 15 {
        37 + (level - 15) * 5
    } else {
        7 + level * 2
    }
}

/// The points it takes to climb from level 0 to `level`: the sum of
/// [`xp_needed_for_next_level`] below it, in closed form.
fn points_to_reach(level: i32) -> i64 {
    let l = i64::from(level.max(0));
    // Sum of 7 + 2k for k in 0..min(l, 15).
    let low = l.min(15);
    let mut total = 7 * low + low * (low - 1);
    if l > 15 {
        // Sum of 37 + 5(k - 15) for k in 15..min(l, 30).
        let n = l.min(30) - 15;
        total += 37 * n + 5 * n * (n - 1) / 2;
    }
    if l > 30 {
        // Sum of 112 + 9(k - 30) for k in 30..l.
        let n = l - 30;
        total += 112 * n + 9 * n * (n - 1) / 2;
    }
    total
}

/// The points a player holds at `level` with `progress` towards the next: the levels' worth plus
/// the bar's (`floor(progress * needed)`, as `/xp query points` counts it). Upstream sums the
/// levels with an integer division that undercounts; this is the game's own count.
pub fn total_points(level: i32, progress: f32) -> i64 {
    let bar = (f64::from(progress) * xp_needed_for_next_level(level) as f64).floor() as i64;
    points_to_reach(level) + bar
}

/// Whether a player at `level` with `progress` towards the next meets cost `rule` ([`ServerSettings::rule`]):
/// the experience check of `DisEnchantingStrategy.hasEnoughExperience` after its creative checks.
pub fn rule_met(rule: i64, level: i32, progress: f32) -> bool {
    match rule {
        0 => true,
        points if points > 0 => total_points(level, progress) >= points,
        levels => i64::from(level) >= -levels,
    }
}

/// [`rule_met`] for the server's own settings.
pub fn has_enough(settings: &ServerSettings, level: i32, progress: f32) -> bool {
    rule_met(settings.rule(), level, progress)
}

// ---- dis-enchanting ---------------------------------------------------------------------------

/// One enchantment: its namespaced id and its level.
pub type Level = (String, u32);

/// What a dis-enchant makes of an input's enchantments (`createResult` and `onTake`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Any item but an enchanted book: every enchantment moves to a new enchanted book, and the
    /// item keeps none.
    Whole { moved: Vec<Level> },
    /// An enchanted book with two or more: its first enchantment goes to a new book, and the input
    /// becomes a new enchanted book with the rest.
    Split { kept: Level, rest: Vec<Level> },
}

/// `DisEnchantingHelper.canDisenchant` and the plan for it: an item with enchantments, or an
/// enchanted book with at least two. `enchantments` are the ones `getEnchantmentsForCrafting`
/// reads (a book's stored ones, any other item's own), in the stack's order.
pub fn plan(is_enchanted_book: bool, enchantments: &[Level]) -> Option<Plan> {
    if !is_enchanted_book {
        return (!enchantments.is_empty()).then(|| Plan::Whole {
            moved: enchantments.to_vec(),
        });
    }
    let (kept, rest) = enchantments.split_first()?;
    (!rest.is_empty()).then(|| Plan::Split {
        kept: kept.clone(),
        rest: rest.to_vec(),
    })
}

impl Plan {
    /// The result slot's stack, as the enchantments of a new enchanted book.
    pub fn result(&self) -> &[Level] {
        match self {
            Self::Whole { moved } => moved,
            Self::Split { kept, .. } => std::slice::from_ref(kept),
        }
    }
}

// ---- particles --------------------------------------------------------------------------------

/// One portal particle of `animateTick`, from the four random draws it makes, in order: two
/// `nextInt(2)` and four `nextFloat` (`y`, then the three speeds). Position and velocity.
pub fn portal_particle(
    pos: [i32; 3],
    x_side: i32,
    z_side: i32,
    floats: [f32; 4],
) -> ([f64; 3], [f64; 3]) {
    let x_mod = f64::from(x_side * 2 - 1);
    let z_mod = f64::from(z_side * 2 - 1);
    let at = [
        f64::from(pos[0]) + 0.5 + 0.25 * x_mod,
        f64::from(pos[1] as f32 + floats[0]),
        f64::from(pos[2]) + 0.5 + 0.25 * z_mod,
    ];
    let velocity = [
        f64::from(floats[1] * x_mod as f32),
        (f64::from(floats[2]) - 0.5) * 0.125,
        f64::from(floats[3] * z_mod as f32),
    ];
    (at, velocity)
}

/// The renderer's turn about the vertical axis for a table facing `facing` (2D data value: 0
/// south, 1 west, 2 north, 3 east): north and south use the opposite side's `toYRot`, east and
/// west their own, as upstream's renderer does.
pub fn item_yaw(facing: &str) -> f32 {
    match facing {
        // Opposite of north is south (0), of south is north (180).
        "north" => 0.0,
        "south" => 180.0,
        "east" => 270.0,
        "west" => 90.0,
        _ => 0.0,
    }
}

/// The renderer's model seed for the table at `pos`: `(int) BlockPos.asLong()`, the low 32 bits of
/// its packed position (26 bits of x, 26 of z, 12 of y).
pub fn pos_seed(pos: [i32; 3]) -> i32 {
    let packed = ((i64::from(pos[0]) & 0x3FF_FFFF) << 38)
        | ((i64::from(pos[2]) & 0x3FF_FFFF) << 12)
        | (i64::from(pos[1]) & 0xFFF);
    packed as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(id: &str, level: u32) -> Level {
        (id.to_owned(), level)
    }

    #[test]
    fn default_files_parse_to_the_defaults() {
        assert_eq!(
            ServerSettings::parse(DEFAULT_SERVER),
            ServerSettings::default()
        );
        assert_eq!(
            ClientSettings::parse(DEFAULT_CLIENT),
            ClientSettings::default()
        );
    }

    #[test]
    fn settings_read_values_and_keep_defaults_for_bad_ones() {
        let s = ServerSettings::parse(
            "automatic_disenchanting = true # on\nexperience_cost = 3_0\nuses_points=false\nrequires_experience = \"no\"\nautomatic_disenchanting_ticks = 99999999999\n",
        );
        assert!(s.automatic_disenchanting);
        assert_eq!(s.experience_cost, 30);
        assert!(!s.uses_points);
        assert!(s.requires_experience);
        assert_eq!(s.automatic_disenchanting_ticks, 10);
        let c = ClientSettings::parse("render_table_item = false\n");
        assert!(!c.render_table_item && c.render_block_particles);
    }

    #[test]
    fn cost_rule_signs_points_and_levels() {
        let mut s = ServerSettings::default();
        assert_eq!(s.cost_rule(), 25);
        s.uses_points = false;
        assert_eq!(s.cost_rule(), -25);
        s.experience_cost = 1_000_000;
        assert_eq!(s.cost_rule(), -32767);
        s.requires_experience = false;
        assert_eq!(s.cost_rule(), 0);
    }

    #[test]
    fn points_match_summing_each_level() {
        let mut sum = 0;
        for level in 0..100 {
            assert_eq!(points_to_reach(level), sum, "level {level}");
            sum += xp_needed_for_next_level(level);
        }
        // Known values: level 16 is 352 points, level 31 is 1507, level 30 is 1395.
        assert_eq!(total_points(16, 0.0), 352);
        assert_eq!(total_points(30, 0.0), 1395);
        assert_eq!(total_points(31, 0.0), 1507);
        // Half of level 1's 9-point bar.
        assert_eq!(total_points(1, 0.5), 7 + 4);
    }

    #[test]
    fn enough_experience_by_points_and_levels() {
        let mut s = ServerSettings::default();
        // 25 points: level 2 holds 16, level 3 holds 27.
        assert!(!has_enough(&s, 2, 0.0));
        assert!(has_enough(&s, 3, 0.0));
        assert!(has_enough(&s, 2, 0.9));
        s.uses_points = false;
        assert!(!has_enough(&s, 24, 0.99));
        assert!(has_enough(&s, 25, 0.0));
        s.experience_cost = 0;
        assert!(has_enough(&s, 0, 0.0));
        s.experience_cost = 25;
        s.requires_experience = false;
        assert!(has_enough(&s, 0, 0.0));
    }

    #[test]
    fn items_move_everything_and_books_split_off_the_first() {
        assert_eq!(plan(false, &[]), None);
        assert_eq!(
            plan(
                false,
                &[lv("minecraft:sharpness", 5), lv("minecraft:unbreaking", 3)]
            ),
            Some(Plan::Whole {
                moved: vec![lv("minecraft:sharpness", 5), lv("minecraft:unbreaking", 3)]
            })
        );
        assert_eq!(plan(true, &[lv("minecraft:mending", 1)]), None);
        let split = plan(
            true,
            &[
                lv("minecraft:mending", 1),
                lv("minecraft:protection", 4),
                lv("minecraft:unbreaking", 3),
            ],
        )
        .unwrap();
        assert_eq!(split.result(), &[lv("minecraft:mending", 1)]);
        assert_eq!(
            split,
            Plan::Split {
                kept: lv("minecraft:mending", 1),
                rest: vec![lv("minecraft:protection", 4), lv("minecraft:unbreaking", 3)],
            }
        );
    }

    #[test]
    fn particles_spread_to_the_four_corners() {
        let (at, v) = portal_particle([10, 64, -3], 0, 1, [0.5, 0.25, 0.0, 1.0]);
        assert_eq!(at, [10.25, 64.5, -2.25]);
        assert_eq!(v, [-0.25, -0.0625, 1.0]);
    }

    #[test]
    fn seed_is_the_packed_position() {
        assert_eq!(pos_seed([0, 0, 0]), 0);
        assert_eq!(pos_seed([0, 64, 0]), 64);
        assert_eq!(pos_seed([0, -1, 0]), 0xFFF);
        assert_eq!(pos_seed([0, 0, 1]), 1 << 12);
        // x lands above bit 38 and leaves the low 32 bits alone.
        assert_eq!(pos_seed([5, 0, 0]), 0);
        assert_eq!(pos_seed([0, 0, -1]), (0x3FF_FFFFi64 << 12) as i32);
    }

    #[test]
    fn item_faces_the_front() {
        assert_eq!(item_yaw("north"), 0.0);
        assert_eq!(item_yaw("south"), 180.0);
        assert_eq!(item_yaw("east"), 270.0);
        assert_eq!(item_yaw("west"), 90.0);
    }
}
