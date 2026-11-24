//! The table in the game: its block entity's tick, its menu's rules, what taking a result does,
//! the screen, the item drawn on top and the particles.
//!
//! Upstream gives each player in manual mode a menu of their own (`ItemCombinerMenu`) and keeps the
//! block entity's three slots for automatic mode. Here both modes use the block entity's slots: in
//! manual mode slot 2 holds a preview of the result (nbt `Preview` says so), the table opens for one
//! player at a time, and closing it gives the inputs back.

use crate::runtime::{self, Level, Plan, ServerSettings};
use crate::{client_settings, keys, server_settings};
use mod_api::{
    AnimateTickEvent, BlockEntityPart, BlockEntitySubmitEvent, BlockEntityTickEvent,
    BlockEntityTicksEvent, BlockEntityUpdateTagEvent, BlockPart, CanPlaceItemEvent,
    CanTakeItemThroughFaceEvent, ClientHost, Component, Compound, DrawContext, Host,
    HostBlockStates, HostTransaction, ItemDisplayContext, ItemStack, JavaString, LevelRef,
    MenuContainer, MenuPart, MenuScreen, MenuScreenFactory, MenuScreenFrame, MenuScreenLayout,
    MenuScreenOpen, MenuSlotRulesEvent, MenuSlotTakeEvent, ParticleOptions, PlayerRef,
    PreRemoveSideEffectsEvent, ScreenSlot, ServerHost, Tag,
};

/// The table's menu type, its block entity type and its block: one id.
pub(crate) const ID: &str = "disenchanting_table:disenchanting_table";
const INPUT: u32 = 0;
const BOOK: u32 = 1;
const OUTPUT: u32 = 2;
/// Data slot 1: [`ServerSettings::cost_rule`]. Data slot 0 holds automatic mode's progress.
const RULE_SLOT: u32 = 1;
/// Level event 1503: the end portal frame's fill, smoke and sound.
const DISENCHANT_EVENT: i32 = 1503;

/// nbt: the manual preview sits in slot 2 (byte 1), or a real item or nothing does.
const PREVIEW: &str = "Preview";
/// nbt: automatic mode's progress, saved as upstream saves it.
const PROGRESS: &str = "Progress";
/// nbt, sent to clients: whether the table runs in automatic mode.
const AUTOMATIC: &str = "Automatic";
/// nbt, sent to clients: the item drawn on top in automatic mode.
const SHOWN: &str = "Shown";

// ---- enchantments ---------------------------------------------------------------------------

/// A component read and written as its raw tag.
struct Raw(&'static str);

impl Component for Raw {
    type Value = Tag;

    fn id(&self) -> &str {
        self.0
    }

    fn decode(&self, value: &Tag) -> Option<Tag> {
        Some(value.clone())
    }

    fn encode(&self, value: Tag) -> Tag {
        value
    }
}

const ENCHANTMENTS: Raw = Raw("minecraft:enchantments");
const STORED_ENCHANTMENTS: Raw = Raw("minecraft:stored_enchantments");
const REPAIR_COST: Raw = Raw("minecraft:repair_cost");

/// `EnchantmentHelper.getEnchantmentsForCrafting`: an enchanted book's stored enchantments, any
/// other item's own, as the stack's patch holds them. No vanilla item has enchantments by default,
/// so the patch is the whole answer, and no host is needed (a hopper's ask has none).
fn enchantments(stack: &ItemStack) -> Vec<Level> {
    let component = if stack.item() == keys().enchanted_book {
        STORED_ENCHANTMENTS
    } else {
        ENCHANTMENTS
    };
    let Some(Tag::Compound(mut levels)) = stack.get(&component) else {
        return Vec::new();
    };
    // The form before 1.21.5 wraps them in `levels`.
    if let Some(Tag::Compound(inner)) = levels.get(&JavaString::from("levels")) {
        levels = inner.clone();
    }
    levels
        .iter()
        .filter_map(|(id, level)| {
            let level = match level {
                Tag::Byte(n) => i32::from(*n),
                Tag::Short(n) => i32::from(*n),
                Tag::Int(n) => *n,
                _ => return None,
            };
            let level = u32::try_from(level)
                .ok()
                .filter(|l| (1..=255).contains(l))?;
            let id = id.to_string_lossy();
            let id = if id.contains(':') {
                id
            } else {
                format!("minecraft:{id}")
            };
            Some((id, level))
        })
        .collect()
}

/// What dis-enchanting `stack` makes, if it can be (`DisEnchantingHelper.canDisenchant`).
fn plan_of(stack: Option<&ItemStack>) -> Option<Plan> {
    let stack = stack?;
    runtime::plan(stack.item() == keys().enchanted_book, &enchantments(stack))
}

/// A new enchanted book holding `levels` (`EnchantmentHelper.createBook` and `setEnchantments`).
fn book(levels: &[Level]) -> ItemStack {
    let mut stored = Compound::new();
    for (id, level) in levels {
        stored.insert(id.as_str().into(), Tag::Int(*level as i32));
    }
    let mut patch = Compound::new();
    patch.insert(STORED_ENCHANTMENTS.0.into(), Tag::Compound(stored));
    let mut stack = ItemStack::of(keys().enchanted_book);
    // Ids and levels in range always decode.
    let _ = stack.apply_components(&patch);
    stack
}

/// The preview slot 2 shows for these inputs: the result, when the input can be dis-enchanted
/// and a book waits beside it (`createResult`).
fn preview(input: Option<&ItemStack>, extra: Option<&ItemStack>) -> Option<ItemStack> {
    if !extra.is_some_and(|book| book.item() == keys().book) {
        return None;
    }
    plan_of(input).map(|plan| book(plan.result()))
}

/// What the input becomes once its result is taken (`onTake`, `AutomaticDisEnchantingProcess`):
/// an item loses its enchantments (and its anvil cost when the server says so), a book becomes a
/// new enchanted book with the enchantments it kept.
fn stripped(input: &ItemStack, plan: &Plan, settings: &ServerSettings) -> ItemStack {
    match plan {
        Plan::Whole { .. } => {
            let mut item = input.clone();
            // Setting either to its default value takes it out of the patch.
            if settings.resets_repair_cost {
                item.remove(&REPAIR_COST);
            }
            item.remove(&ENCHANTMENTS);
            item
        }
        Plan::Split { rest, .. } => book(rest),
    }
}

// ---- experience -----------------------------------------------------------------------------

/// `DisEnchantingStrategy.hasEnoughExperience`.
fn has_enough(host: &dyn Host, player: PlayerRef, settings: &ServerSettings) -> bool {
    if !settings.requires_experience
        || player.abilities(host).is_some_and(|a| a.instabuild)
        || player.is_creative(host) == Some(true)
    {
        return true;
    }
    player
        .experience(host)
        .is_some_and(|xp| runtime::has_enough(settings, xp.level, xp.progress))
}

/// Takes a dis-enchant's cost from `player`, when the server asks for one and the player has it.
/// Points go through `/xp add`, which takes a negative amount: the API's point grant takes only
/// positive ones (sunrise-org/sunrise#5013).
fn charge(host: &mut dyn ServerHost, player: PlayerRef, settings: &ServerSettings) {
    if !settings.requires_experience || !has_enough(&*host, player, settings) {
        return;
    }
    let cost = settings.experience_cost;
    if settings.uses_points {
        let uuid = player.uuid();
        let uuid = format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            uuid >> 96,
            (uuid >> 80) & 0xFFFF,
            (uuid >> 64) & 0xFFFF,
            (uuid >> 48) & 0xFFFF,
            uuid & 0xFFFF_FFFF_FFFF
        );
        host.commands()
            .run_command(&format!("xp add {uuid} {} points", -i64::from(cost)));
    } else {
        player.give_levels(host, -cost);
    }
}

// ---- the block entity -----------------------------------------------------------------------

fn flag(nbt: &Compound, key: &str) -> bool {
    matches!(nbt.get(&JavaString::from(key)), Some(Tag::Byte(1)))
}

fn byte(on: bool) -> Option<Tag> {
    Some(Tag::Byte(i8::from(on)))
}

/// The nearest player within 16 blocks of the table's corner who is not a spectator
/// (`Level.getNearestPlayer(x, y, z, 16, false)`).
fn nearest_player(host: &dyn ServerHost, level: LevelRef, pos: [i32; 3]) -> Option<PlayerRef> {
    let corner = pos.map(f64::from);
    let mut best: Option<(f64, PlayerRef)> = None;
    for player in host.players().server_players() {
        if player.level(host) != Some(level) || player.is_spectator(host) != Some(false) {
            continue;
        }
        let Some(at) = player.entity().position(host) else {
            continue;
        };
        let dist = (0..3).map(|i| (at[i] - corner[i]).powi(2)).sum::<f64>();
        if dist < 16.0 * 16.0 && best.is_none_or(|(b, _)| dist < b) {
            best = Some((dist, player));
        }
    }
    best.map(|(_, player)| player)
}

/// The table's tick. In manual mode it keeps slot 2's preview in step with the inputs; in
/// automatic mode it is upstream's `tick`: the nearest player pays, and every
/// `automatic_disenchanting_ticks` a valid input is dis-enchanted into slot 2.
fn tick(event: &mut BlockEntityTickEvent<'_>) {
    let settings = server_settings();
    let scope = &mut event.subject;
    let Some((level, pos, items, nbt, data)) = scope.with_view(|view| {
        let items: [Option<ItemStack>; 3] =
            std::array::from_fn(|i| view.item_slot(i as u32).and_then(|s| s.to_owned()));
        (
            view.level(),
            view.position(),
            items,
            view.nbt().clone(),
            view.data_slots().to_vec(),
        )
    }) else {
        return;
    };
    let was_preview = flag(&nbt, PREVIEW);
    let mut new_items = items.clone();
    let mut progress = match nbt.get(&JavaString::from(PROGRESS)) {
        Some(Tag::Int(p)) => *p,
        _ => 0,
    };
    let mut is_preview = was_preview;
    let mut disenchanted = None;
    if !settings.automatic_disenchanting {
        progress = 0;
        // A real item in slot 2 (left from automatic mode) waits to be taken.
        if items[2].is_none() || was_preview {
            new_items[2] = preview(items[0].as_ref(), items[1].as_ref());
            is_preview = new_items[2].is_some();
        }
    } else {
        if was_preview {
            new_items[2] = None;
            is_preview = false;
        }
        let host = scope.host();
        let player = nearest_player(&*host, level, pos)
            .filter(|&player| has_enough(&*host, player, &settings));
        let plan = plan_of(new_items[0].as_ref());
        match (player, plan) {
            (Some(player), Some(plan))
                if new_items[1]
                    .as_ref()
                    .is_some_and(|b| b.item() == keys().book)
                    && new_items[2].is_none() =>
            {
                if progress >= settings.automatic_disenchanting_ticks {
                    let input = new_items[0].take().expect("a plan has an input");
                    new_items[2] = Some(book(plan.result()));
                    new_items[0] = Some(stripped(&input, &plan, &settings));
                    new_items[1] = new_items[1].take().and_then(|b| b.shrink(1));
                    disenchanted = Some(player);
                    progress = 0;
                }
                progress += 1;
            }
            (Some(_), _) => progress = 0,
            (None, _) => progress = 0,
        }
    }
    if let Some(player) = disenchanted {
        let host = scope.host();
        charge(host, player, &settings);
        level.level_event(host, DISENCHANT_EVENT, pos, 0);
    }
    let shown = if settings.automatic_disenchanting {
        new_items[0]
            .as_ref()
            .and_then(|stack| stack.to_nbt(&*scope.host()))
    } else {
        None
    };
    let rule = settings.cost_rule();
    let mut sync = false;
    scope.with_view(|mut view| {
        for (slot, (old, new)) in items.iter().zip(&new_items).enumerate() {
            if old != new {
                let _ = view.set_item_slot(slot as u32, new.clone());
            }
        }
        let wanted = [progress, rule];
        if data.as_slice() != wanted {
            view.set_data_slots(&wanted);
        }
        if is_preview != was_preview {
            view.set_nbt(PREVIEW.into(), byte(is_preview));
        }
        let saved = settings
            .automatic_disenchanting
            .then_some(Tag::Int(progress));
        if nbt.get(&JavaString::from(PROGRESS)) != saved.as_ref() {
            view.set_nbt(PROGRESS.into(), saved);
        }
        if flag(&nbt, AUTOMATIC) != settings.automatic_disenchanting {
            view.set_nbt(AUTOMATIC.into(), byte(settings.automatic_disenchanting));
            sync = true;
        }
        let shown = shown.map(Tag::Compound);
        if nbt.get(&JavaString::from(SHOWN)) != shown.as_ref() {
            view.set_nbt(SHOWN.into(), shown);
            sync = true;
        }
    });
    if sync {
        scope.host().block_entity_sync(pos);
    }
}

/// The block entity a player's open menu at `pos` stands on, when it is this table.
fn table_at(host: &dyn Host, player: PlayerRef, pos: [i32; 3]) -> Option<mod_api::BlockEntityRef> {
    let level = player.level(host)?;
    let entity = host.block_entity(level, pos)?;
    (entity.kind(host)? == keys().kind).then_some(entity)
}

/// The preview flag of the table a player has open, read from its saved data.
fn preview_shown(host: &dyn Host, player: PlayerRef, pos: [i32; 3]) -> Option<bool> {
    let entity = table_at(host, player, pos)?;
    Some(flag(&entity.data(host)?, PREVIEW))
}

/// Manual mode, as the table's menu closes: the closing player gets the inputs back
/// (`ItemCombinerMenu.removed`), and the preview goes. A player whose inventory is full gets
/// them dropped at their feet.
pub(crate) fn close(host: &mut dyn ServerHost, player: u128, pos: Option<[i32; 3]>) {
    if server_settings().automatic_disenchanting {
        return;
    }
    let Some(pos) = pos else {
        return;
    };
    let Some(player) = host.player(player) else {
        return;
    };
    let Some(entity) = table_at(&*host, player, pos) else {
        return;
    };
    let shows_preview = entity.data(&*host).is_some_and(|data| flag(&data, PREVIEW));
    let mut returned = Vec::new();
    let _ = host.transaction(|host, tx| {
        for slot in [INPUT, BOOK] {
            if let Ok(Some(stack)) = entity.item_slot(&*host, slot) {
                entity.set_item_slot(host, tx, slot, None).map_err(|_| ())?;
                returned.push(stack);
            }
        }
        if shows_preview {
            entity
                .set_item_slot(host, tx, OUTPUT, None)
                .map_err(|_| ())?;
        }
        Ok::<(), ()>(())
    });
    for stack in returned {
        if let Ok(Some(left)) = player.give_stack(host, stack) {
            let _ = player.drop_stack(host, left, false);
        }
    }
}

/// Manual mode, as a player opens the table: refused while another player has it open, since
/// the inputs are the table's own slots.
pub(crate) fn in_use_by_another(host: &dyn ServerHost, player: u128, pos: [i32; 3]) -> bool {
    if server_settings().automatic_disenchanting {
        return false;
    }
    let Some(player) = host.player(player) else {
        return false;
    };
    let Some(entity) = table_at(host, player, pos) else {
        return false;
    };
    entity
        .viewers(host)
        .is_some_and(|viewers| viewers.iter().any(|v| v.uuid() != player.uuid()))
}

pub(crate) struct Table;

#[mod_api::sunrise_part]
impl BlockPart for Table {
    /// `animateTick`: three portal particles that drift out from the table's corners.
    fn animate_tick(&self, host: &mut dyn ClientHost, event: &mut AnimateTickEvent<'_>) {
        if !client_settings().render_block_particles {
            return;
        }
        let p = event.pos;
        let portal = ParticleOptions::simple(keys().portal);
        for _ in 0..3 {
            let x_side = event.random.next_int_bound(2);
            let z_side = event.random.next_int_bound(2);
            let floats = std::array::from_fn(|_| event.random.next_float());
            let (at, velocity) = runtime::portal_particle([p.x, p.y, p.z], x_side, z_side, floats);
            host.add_particle(&portal, at, velocity);
        }
    }
}

#[mod_api::sunrise_part]
impl BlockEntityPart for Table {
    fn block_entity_ticks(&self, _: &dyn Host, event: &mut BlockEntityTicksEvent<'_>) {
        event.ticks = true;
    }

    fn block_entity_tick(&self, event: &mut BlockEntityTickEvent<'_>) {
        tick(event);
    }

    /// Hoppers fill the input and the book in automatic mode only.
    fn can_place_item(&self, event: &mut CanPlaceItemEvent<'_>) -> Option<bool> {
        let allowed = server_settings().automatic_disenchanting
            && match event.slot as u32 {
                INPUT => plan_of(Some(event.stack)).is_some(),
                BOOK => event.stack.item() == keys().book,
                _ => false,
            };
        Some(allowed)
    }

    /// `canTakeItemThroughFace`: any slot in automatic mode, as upstream. Manual mode keeps the
    /// open player's inputs and preview in the table, where upstream's menu held them, so
    /// nothing leaves through a side.
    fn can_take_item_through_face(
        &self,
        _: &dyn Host,
        _: &mut CanTakeItemThroughFaceEvent<'_>,
    ) -> Option<bool> {
        Some(server_settings().automatic_disenchanting)
    }

    /// Breaking the table drops what it holds; in manual mode the preview is not an item.
    fn pre_remove_side_effects(
        &self,
        _: &mut dyn mod_api::LevelRandomHost,
        event: &mut PreRemoveSideEffectsEvent<'_>,
    ) {
        if flag(&event.entity.nbt, PREVIEW) {
            event.keep_contents = true;
            let items = &event.entity.items;
            for slot in [INPUT, BOOK] {
                event
                    .drops
                    .push(items.get(slot as usize).cloned().flatten());
            }
        }
    }

    /// What clients get: the mode and, in automatic mode, the item on top.
    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        let mut tag = Compound::new();
        for key in [AUTOMATIC, SHOWN] {
            if let Some(value) = event.entity.nbt.get(&JavaString::from(key)) {
                tag.insert(key.into(), value.clone());
            }
        }
        Some(tag)
    }

    /// Upstream's renderer: an enchanted book lies on the table in manual mode, the input in
    /// automatic mode, half size, flat, turned to the table's front.
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if !client_settings().render_table_item {
            return;
        }
        let nbt = &event.entity.nbt;
        let stack = if flag(nbt, AUTOMATIC) {
            match nbt.get(&JavaString::from(SHOWN)) {
                Some(Tag::Compound(shown)) => ItemStack::from_nbt(host, shown),
                _ => None,
            }
        } else {
            Some(ItemStack::of(keys().enchanted_book))
        };
        let Some(stack) = stack else {
            return;
        };
        let states = host.block_states();
        let facing = event
            .state
            .property(&states, "facing")
            .ok()
            .flatten()
            .unwrap_or("north");
        let draw = &mut *event.draw;
        draw.push();
        draw.translate([0.5, 0.75, 0.5]);
        draw.scale([0.5, 0.5, 0.5]);
        draw.rotate_about([0.0, 1.0, 0.0], runtime::item_yaw(facing), [0.0, 0.0, 0.0]);
        draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
        draw.item(
            &stack,
            ItemDisplayContext::Fixed,
            None,
            runtime::pos_seed(event.pos),
        );
        draw.pop();
    }
}

// ---- the menu -------------------------------------------------------------------------------

pub(crate) struct TableMenu;

#[mod_api::sunrise_part]
impl MenuPart for TableMenu {
    /// Manual mode's `onTake`: the input loses what the result took, a book is used, the player
    /// pays, and the table shows the next preview.
    fn menu_slot_take(&self, event: &mut MenuSlotTakeEvent<'_>) {
        let settings = server_settings();
        if event.slot != OUTPUT as usize || settings.automatic_disenchanting {
            return;
        }
        let scope = &mut event.subject;
        let Some((level, pos, input, extra, was_preview)) = scope.with_view(|view| {
            (
                view.level(),
                view.position(),
                view.item_slot(INPUT).and_then(|s| s.to_owned()),
                view.item_slot(BOOK).and_then(|s| s.to_owned()),
                flag(view.nbt(), PREVIEW),
            )
        }) else {
            return;
        };
        // A real item left from automatic mode is just taken.
        let Some(plan) = was_preview.then(|| plan_of(input.as_ref())).flatten() else {
            return;
        };
        let input = input.map(|i| stripped(&i, &plan, &settings));
        let extra = extra.and_then(|b| b.shrink(1));
        let next = preview(input.as_ref(), extra.as_ref());
        scope.with_view(|mut view| {
            let _ = view.set_item_slot(INPUT, input);
            let _ = view.set_item_slot(BOOK, extra);
            let _ = view.set_item_slot(OUTPUT, next.clone());
            view.set_nbt(PREVIEW.into(), byte(next.is_some()));
        });
        let host = scope.host();
        if let Some(player) = host.player(event.player) {
            charge(host, player, &settings);
        }
        if client_settings().render_block_particles {
            level.level_event(host, DISENCHANT_EVENT, pos, 0);
        }
    }

    /// The slots' rules, the same in both modes: slot 0 takes what can be dis-enchanted, slot 1
    /// books, slot 2 nothing. In manual mode the preview is taken only by a player with the
    /// experience for it, and only while the inputs still make it.
    fn menu_slot_rules(&self, host: &dyn Host, event: &mut MenuSlotRulesEvent<'_>) {
        if event.container != MenuContainer::Main {
            return;
        }
        match event.slot as u32 {
            INPUT => event.may_place = plan_of(Some(event.stack)).is_some(),
            BOOK => event.may_place = event.stack.item() == keys().book,
            _ => {
                event.may_place = false;
                let settings = server_settings();
                if settings.automatic_disenchanting {
                    return;
                }
                let context = event.context;
                let (Some(player), Some(pos)) = (
                    context.player.and_then(|uuid| host.player(uuid)),
                    context.pos,
                ) else {
                    return;
                };
                // The client may not see the flag: it predicts a preview, the server corrects.
                if preview_shown(host, player, pos) == Some(false) {
                    return;
                }
                let input = context.slot(MenuContainer::Main, INPUT as usize);
                let extra = context.slot(MenuContainer::Main, BOOK as usize);
                let made = preview(input.as_ref(), extra.as_ref()).as_ref() == Some(event.stack);
                event.may_pickup = made && has_enough(host, player, &settings);
            }
        }
    }
}

// ---- the screen -----------------------------------------------------------------------------

/// The table's screen: upstream's background, no labels, and the red notice when the player
/// can't pay.
pub(crate) struct TableScreen;

impl MenuScreenFactory for TableScreen {
    fn create(&self, _: &dyn ClientHost, _: MenuScreenOpen<'_>) -> Option<MenuScreen> {
        let slot = |x: f32| {
            Some(ScreenSlot {
                position: [x, 47.0],
                ..Default::default()
            })
        };
        let layout = MenuScreenLayout {
            width: 176.0,
            height: 166.0,
            slots: vec![slot(27.0), slot(76.0), slot(134.0)],
            inventory_x: 8.0,
            inventory_y: 84.0,
            own_labels: true,
            own_background: true,
            ..Default::default()
        };
        Some(MenuScreen::new(layout).with_draw(screen_draw))
    }
}

fn screen_draw(host: &dyn ClientHost, frame: MenuScreenFrame<'_>, draw: &mut dyn DrawContext) {
    let [x, y] = frame.origin;
    draw.sprite(
        "disenchanting_table:gui/container/disenchanting_table",
        [0.0, 0.0, 176.0, 166.0],
        [x, y, 176.0, 166.0],
        None,
    );
    if !client_settings().render_experience_cost {
        return;
    }
    let Some(player) = host.local_player() else {
        return;
    };
    let rule = frame.data.get(RULE_SLOT as usize).copied().unwrap_or(0);
    let free = player.abilities(host).is_some_and(|a| a.instabuild)
        || player.is_creative(host) == Some(true);
    let enough = player
        .experience(host)
        .is_some_and(|xp| runtime::rule_met(i64::from(rule), xp.level, xp.progress));
    if free || enough {
        return;
    }
    // Upstream's literal text and colours.
    let text = "Insufficient Experience!";
    let (left, top) = (x + 45.0, y + 72.0);
    draw.fill([left, top, draw.text_width(text) + 4.0, 11.0], 0xFF45_327F);
    draw.text(text, [left + 2.0, top + 2.0], 0xFFFF_6060, true);
}
