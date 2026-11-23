# Dis-Enchanting Table for Sunrise

A table that moves an item's enchantments onto a book, or splits an enchanted book in two.

A port of [Dis-Enchanting Table](https://github.com/Mods-For-Lupin/Dis-Enchanting-Redux) by Lupin and Jason13 to [Sunrise](https://playsunrise.net) (Minecraft Java 26.3).

## What it does

- A Dis-Enchanting Table block, crafted and broken as in the original, with its portal particles drifting out of the table.
- Put an enchanted item and a book in the table and take an enchanted book holding all of the item's enchantments; the item keeps everything else and, by default, loses its anvil repair cost.
- An enchanted book with more than one enchantment splits in two: its first enchantment moves to the new book and it keeps the rest.
- Taking the book costs 25 experience points by default; a player without them sees "Insufficient Experience!" and can't take it. Creative players pay nothing.
- Automatic mode (server setting): the table dis-enchants on its own every 10 ticks, charging the nearest player within 16 blocks, and hoppers can fill its input and book slots. The item being worked on lies on top of the table.
- The original's server settings (`disenchanting_table-server.toml`: automatic mode and its speed, repair-cost reset, whether and how much experience it takes, points or levels) and client settings (`disenchanting_table-client.toml`: particles, the experience notice, the item on top).
- The original's recipe, loot table, creative tab entry, models, textures and English, Japanese, Russian and Turkish translations.

## Differences from the original

- In manual mode the table opens for one player at a time, and the items you put in are the table's own slots: closing the table gives them back (or drops them if your inventory is full), as the original does.
- The result appears in the right-hand slot on the table's next tick rather than the moment you place the item.
- The experience check counts your points exactly; the original's count could be off by a few points at some levels.
- The item on top of the table and the automatic-mode setting now show the same on every player's screen in multiplayer.
- Hoppers can't take the finished book out yet: that waits on a mod API addition (sunrise-org/sunrise#5014). They can fill the table.

## Building

With the Sunrise mod tool (`cargo sunrise-mod`, from the Sunrise SDK) and Rust:

```sh
cargo sunrise-mod build    # compiles against the pinned game, packs target/sunrise-mod/disenchanting_table.smod
cargo sunrise-mod run      # builds it and starts the game with it
cargo test
```

## Licence

Unlicense, as the original. See [LICENSE](LICENSE) and [LICENSE-NOTICE.md](LICENSE-NOTICE.md).
