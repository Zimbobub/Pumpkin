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
pub trait CraftingScreenHandler<I: RecipeInputInventory>:
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


// CraftingMenu
pub struct WorkstationScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    crafting_inventory: Arc<dyn RecipeInputInventory>,
}

impl WorkstationScreenHandler {
    pub async fn new(sync_id: u8, player_inventory: &Arc<PlayerInventory>, window_type: WindowType, width: u8, height: u8) -> Self {
        let crafting_inventory: Arc<dyn RecipeInputInventory> =
            Arc::new(WorkstationInventory::new(width, height));

        let mut workstation_screen_handler = WorkstationScreenHandler {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(window_type)),
            crafting_inventory: crafting_inventory.clone(),
        };

        workstation_screen_handler
            .add_recipe_slots(crafting_inventory)
            .await;

        // Add player inventory slots
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        workstation_screen_handler.add_player_slots(&player_inventory);

        workstation_screen_handler
    }

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

impl RecipeFinderScreenHandler for WorkstationScreenHandler {}

impl ScreenHandler for WorkstationScreenHandler {
    fn on_closed<'a>(&'a mut self, player: &'a dyn InventoryPlayer) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            self.default_on_closed(player).await;
            //TODO: this.craftingResultInventory.clear();
            self.drop_inventory(player, self.crafting_inventory.clone())
                .await;
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_behaviour(&self) -> &ScreenHandlerBehaviour {
        &self.behaviour
    }

    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour {
        &mut self.behaviour
    }

    fn quick_move<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer,
        slot_index: i32,
    ) -> ItemStackFuture<'a> {
        Box::pin(async move {
            let slot = self.get_behaviour().slots[slot_index as usize].clone();

            if slot.has_stack().await {
                let slot_stack = slot.get_stack().await;
                let mut slot_stack = slot_stack.lock().await;
                let stack_prev = slot_stack.clone();

                if slot_index == 0 {
                    // From crafting result slot - move to player inventory (slots 10-46)
                    if !self.insert_item(&mut slot_stack, 10, 46, true).await {
                        return ItemStack::EMPTY.clone();
                    }
                } else if (1..=9).contains(&slot_index) {
                    // From crafting input slots - try to move to player inventory (slots 10-46)
                    if !self.insert_item(&mut slot_stack, 10, 46, false).await {
                        return ItemStack::EMPTY.clone();
                    }
                } else if (10..46).contains(&slot_index) {
                    // From player inventory - try to move to crafting input slots first (1-9)
                    if !self.insert_item(&mut slot_stack, 1, 10, false).await {
                        // If that fails, try moving within player inventory
                        if slot_index < 37 {
                            // From main inventory to hotbar
                            if !self.insert_item(&mut slot_stack, 37, 46, false).await {
                                return ItemStack::EMPTY.clone();
                            }
                        } else {
                            // From hotbar to main inventory
                            if !self.insert_item(&mut slot_stack, 10, 37, false).await {
                                return ItemStack::EMPTY.clone();
                            }
                        }
                    }
                } else {
                    // Any other slot - try to move to player inventory
                    if !self.insert_item(&mut slot_stack, 10, 46, false).await {
                        return ItemStack::EMPTY.clone();
                    }
                }

                let stack = slot_stack.clone();
                drop(slot_stack); // release the lock before calling other methods

                if stack.is_empty() {
                    slot.set_stack_prev(ItemStack::EMPTY.clone(), stack_prev.clone())
                        .await;
                } else {
                    slot.mark_dirty().await;
                }

                if stack.item_count == stack_prev.item_count {
                    // Nothing changed
                    return ItemStack::EMPTY.clone();
                }

                slot.on_take_item(player, &stack).await;

                if slot_index == 0 {
                    slot.on_quick_move_crafted(stack.clone(), stack_prev.clone())
                        .await;
                    // For crafting result slot, drop any remaining items
                    if !stack.is_empty() {
                        player.drop_item(stack, false).await;
                    }
                }

                return stack_prev;
            }

            ItemStack::EMPTY.clone()
        })
    }
}

impl CraftingScreenHandler<WorkstationInventory> for WorkstationScreenHandler {}
