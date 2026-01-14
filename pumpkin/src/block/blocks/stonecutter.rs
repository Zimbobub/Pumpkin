use crate::block::registry::BlockActionResult;
use crate::block::{BlockBehaviour, BlockFuture, NormalUseArgs};

use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_data::screen::WindowType;
use pumpkin_inventory::workstation::crafting_table_screen_handler::CraftingTableScreenHandler;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::text::TextComponent;
use std::sync::Arc;
use tokio::sync::Mutex;

#[pumpkin_block("minecraft:stonecutter")]
pub struct StonecutterBlock;

impl BlockBehaviour for StonecutterBlock {
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            args.player
                .open_handled_screen(&StonecutterScreenFactory)
                .await;

            BlockActionResult::Success
        })
    }
}

struct StonecutterScreenFactory;

impl ScreenHandlerFactory for StonecutterScreenFactory {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<PlayerInventory>,
        _player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            let handler = CraftingTableScreenHandler::new(sync_id, player_inventory/*, 1, 1, WindowType::Stonecutter*/).await;
            let concrete_arc = Arc::new(Mutex::new(handler));

            Some(concrete_arc as SharedScreenHandler)
        })
    }

    fn get_display_name(&self) -> TextComponent {
        TextComponent::translate("container.stonecutter", &[])
    }
}
