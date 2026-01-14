use std::any::Any;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::recipes::{RecipeFinderScreenHandler, RecipeInputInventory};
use crate::workstation::result_slot::ResultSlot;
use crate::workstation::workstation_inventory::WorkstationInventory;
use crate::player::player_inventory::PlayerInventory;
use crate::screen_handler::{
    InventoryPlayer, ItemStackFuture, ScreenHandler, ScreenHandlerBehaviour, ScreenHandlerFuture,
    ScreenHandlerListener,
};
use crate::slot::{BoxFuture, NormalSlot, Slot};

use crossbeam_utils::atomic::AtomicCell;
use pumpkin_data::recipes::{CraftingRecipeTypes, RECIPES_CRAFTING, RecipeResultStruct};
use pumpkin_data::screen::WindowType;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::item::ItemStack;
use tokio::sync::Mutex;

// AbstractCraftingScreenHandler.java
pub trait WorkstationScreenHandler<I: RecipeInputInventory>:
    RecipeFinderScreenHandler + ScreenHandler
{
    fn add_recipe_slots<'a>(
        &'a mut self,
        crafing_inventory: Arc<dyn RecipeInputInventory>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let result_slot = Arc::new(ResultSlot::new(crafing_inventory.clone()));
            self.add_slot(result_slot.clone());

            let width = crafing_inventory.get_width();
            let height = crafing_inventory.get_height();
            for i in 0..width {
                for j in 0..height {
                    // Assuming j + i * width is the correct slot index calculation
                    let input_slot = NormalSlot::new(crafing_inventory.clone(), j + i * width);
                    self.add_slot(Arc::new(input_slot));
                }
            }

            self.add_listener(result_slot).await;
        })
    }
}
