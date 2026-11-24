//! Dis-Enchanting Table for Sunrise: the edges between the game (`mod_api`) and the model
//! (`runtime.rs`). The block, its block entity, its menu and screen are declared here; what they do
//! is in `table.rs`.

mod runtime;
mod table;

use mod_api::{
    Block, BlockEntityTypeKey, BlockKey, BlockProperties, BlockShapes, BlocksEvent,
    Config as ModConfig, ConfigContext, ConfigKind, ConfigLoadingEvent, ConfigReloadingEvent, Flow,
    Hooks, HostSide, InteractionResult, ItemKey, ModContainer, NoteInstrument, ParticleKey,
    PlayerContainerCloseEvent, RegisterConfigsEvent, Registrar, RegistrationContext,
    RightClickBlockEvent, ServerHost,
};
use runtime::{ClientSettings, ServerSettings};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

const MOD: &str = "disenchanting_table";

/// The registry keys the mod asks about, looked up once at registration.
pub(crate) struct Keys {
    pub kind: BlockEntityTypeKey,
    pub book: ItemKey,
    pub enchanted_book: ItemKey,
    pub portal: ParticleKey,
}

static KEYS: OnceLock<Keys> = OnceLock::new();

pub(crate) fn keys() -> &'static Keys {
    KEYS.get()
        .expect("the mod's keys are looked up at registration")
}

static SERVER: Mutex<ServerSettings> = Mutex::new(ServerSettings {
    automatic_disenchanting: false,
    automatic_disenchanting_ticks: 10,
    resets_repair_cost: true,
    requires_experience: true,
    uses_points: true,
    experience_cost: 25,
});
static CLIENT: Mutex<ClientSettings> = Mutex::new(ClientSettings {
    render_block_particles: true,
    render_experience_cost: true,
    render_table_item: true,
});

/// The server file's settings (`disenchanting_table-server.toml`).
pub(crate) fn server_settings() -> ServerSettings {
    *SERVER.lock().unwrap_or_else(|e| e.into_inner())
}

/// The client file's settings (`disenchanting_table-client.toml`).
pub(crate) fn client_settings() -> ClientSettings {
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner())
}

fn apply(mod_id: &str, kind: ConfigKind, text: &str) {
    if mod_id != MOD {
        return;
    }
    match kind {
        ConfigKind::Server => {
            *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = ServerSettings::parse(text)
        }
        ConfigKind::Client => {
            *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = ClientSettings::parse(text)
        }
        _ => {}
    }
}

#[derive(Default)]
struct Mod;

#[mod_api::sunrise_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
        let block: BlockKey = r.block_key(table::ID);
        let kind = r.block_entity_type(table::ID).blocks(&[table::ID]).build();
        // A block entity Part also answers for its blocks, particles included.
        r.block_entity_part(&kind, table::Table)
            .expect("the table's block entity type is the mod's own");
        // Every face reaches every slot (`getSlotsForFace`): hoppers fill the input and the
        // book, and in automatic mode take from any slot (`Table::can_take_item_through_face`).
        r.block_container(
            &block,
            ModContainer {
                slots: 3,
                faces: std::array::from_fn(|_| vec![0, 1, 2]),
                inputs: vec![0, 1],
                outputs: vec![0, 1, 2],
                menu: Some(table::ID.to_owned()),
                ..Default::default()
            },
        );
        r.menu_part(table::ID, table::TableMenu);
        r.register_menu_screen(table::ID, std::sync::Arc::new(table::TableScreen))
            .expect("the table's menu is the mod's own");
        let _ = KEYS.set(Keys {
            kind,
            book: r.item_key("minecraft:book"),
            enchanted_book: r.item_key("minecraft:enchanted_book"),
            portal: r.particle_key("minecraft:portal"),
        });
    }

    /// The table: the enchanting table's properties (`Properties.ofFullCopy(ENCHANTING_TABLE)`),
    /// a furnace's facing, and upstream's 16 x 12 x 16 shape.
    fn blocks(&self, _: &RegistrationContext, event: &mut BlocksEvent<'_>) {
        let mut block = Block::new(table::ID, "minecraft:furnace").with_item(true);
        block.properties = BlockProperties {
            hardness: Some(5.0),
            explosion_resistance: Some(1200.0),
            light: Some(7),
            requires_tool: Some(true),
            sounds: Some("minecraft:stone".into()),
            // MapColor.COLOR_RED.
            map_color: Some(28),
            instrument: Some(NoteInstrument::Basedrum),
            occludes: Some(true),
            ..Default::default()
        };
        let shape = BlockShapes {
            properties: Vec::new(),
            values: BTreeMap::new(),
            default: Some(vec![[0.0, 0.0, 0.0, 1.0, 0.75, 1.0]]),
        };
        block.shapes = Some(shape.clone());
        block.collision_shapes = Some(shape);
        // A refused declaration is reported at startup with its reason.
        let _ = event.register(block);
    }

    fn register_configs(&self, _: &dyn ConfigContext, event: &mut RegisterConfigsEvent<'_>) {
        event.configs.push(ModConfig::new(
            MOD,
            ConfigKind::Server,
            runtime::DEFAULT_SERVER,
        ));
        event.configs.push(ModConfig::new(
            MOD,
            ConfigKind::Client,
            runtime::DEFAULT_CLIENT,
        ));
    }

    fn config_loading(&self, _: &dyn ConfigContext, event: &mut ConfigLoadingEvent<'_>) {
        apply(
            event.config.mod_id.as_str(),
            event.config.kind,
            &event.config.text,
        );
    }

    fn config_reloading(&self, _: HostSide, event: &mut ConfigReloadingEvent<'_>) {
        apply(
            event.config.mod_id.as_str(),
            event.config.kind,
            &event.config.text,
        );
    }

    /// Manual mode: the table opens for one player at a time.
    fn right_click_block(&self, host: HostSide<'_>, event: &mut RightClickBlockEvent<'_>) -> Flow {
        let HostSide::Server(host) = host else {
            return Flow::Continue;
        };
        if table::in_use_by_another(&*host, event.click.player, event.click.pos) {
            *event.cancellation_result = InteractionResult::Fail;
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn player_container_close(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerContainerCloseEvent<'_>,
    ) {
        table::close(host, event.player, event.pos);
    }
}
