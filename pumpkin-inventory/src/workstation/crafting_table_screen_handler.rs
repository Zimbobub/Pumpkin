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
use crate::workstation::workstation_screen_handler::CraftingScreenHandler;

use crossbeam_utils::atomic::AtomicCell;
use pumpkin_data::recipes::{CraftingRecipeTypes, RECIPES_CRAFTING, RecipeResultStruct};
use pumpkin_data::screen::WindowType;
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_world::inventory::Inventory;
use pumpkin_world::item::ItemStack;
use tokio::sync::Mutex;


fn is_symmetrical_horizontally(pattern: &'static [&'static str]) -> bool {
    let width = pattern.first().map_or(0, |s| s.len());
    for row in pattern {
        if row.len() != width {
            return false; // All rows must have the same length
        }
        for j in 0..width / 2 {
            if row.chars().nth(j) != row.chars().nth(width - j - 1) {
                return false; // Characters must match symmetrically
            }
        }
    }
    true
}

async fn recipe_matches<'a>(
    recipe: &'static CraftingRecipeTypes,
    input_height: usize,
    input_width: usize,
    top_x: usize,
    top_y: usize,
    count: usize,
    inventory: &'a dyn RecipeInputInventory,
) -> Option<&'a RecipeResultStruct> {
    match recipe {
        CraftingRecipeTypes::CraftingShaped {
            key,
            pattern,
            result,
            ..
        } => {
            if pattern.len() != input_height || pattern.first().unwrap().len() != input_width {
                return None;
            }

            if count
                != pattern
                    .iter()
                    .map(|l| l.chars().filter(|c| *c != ' ').count())
                    .sum::<usize>()
            {
                return None;
            }

            let x_offset = top_x;
            let y_offset = top_y;

            let mut matched = true;
            'outer: for y in 0..pattern.len() {
                for x in 0..pattern[y].len() {
                    let current_key = pattern[y].chars().nth(x).unwrap();
                    let slot = inventory
                        .get_stack((y + y_offset) * inventory.get_height() + (x + x_offset))
                        .await;
                    if current_key == ' ' {
                        if !slot.lock().await.is_empty() {
                            matched = false;
                            break 'outer;
                        }
                        continue;
                    }

                    let ingredient = key
                        .iter()
                        .find_map(|(k, v)| (*k == current_key).then_some(v))
                        .expect("Crafting recipe used invalid key");

                    let slot = slot.lock().await;

                    if !ingredient.match_item(slot.item) {
                        matched = false;
                        break 'outer;
                    }
                }
            }

            // Check for asymmetrical recipes
            if !matched && !is_symmetrical_horizontally(pattern) {
                matched = true;
                'outer: for y in 0..pattern.len() {
                    for x in 0..pattern[y].len() {
                        let current_key = pattern[y].chars().nth(x).unwrap();

                        let slot = inventory
                            .get_stack(
                                (y + y_offset) * inventory.get_height()
                                    + (x_offset + input_width - 1 - x),
                            )
                            .await;
                        if current_key == ' ' {
                            if !slot.lock().await.is_empty() {
                                matched = false;
                                break 'outer;
                            }
                            continue;
                        }

                        let ingredient = key
                            .iter()
                            .find_map(|(k, v)| (*k == current_key).then_some(v))
                            .expect("Crafting recipe used invalid key");

                        let slot = slot.lock().await;

                        if !ingredient.match_item(slot.item) {
                            matched = false;
                            break 'outer;
                        }
                    }
                }
            }

            // TODO: Apply components
            if matched { Some(result) } else { None }
        }
        CraftingRecipeTypes::CraftingShapeless {
            ingredients,
            result,
            ..
        } => {
            if count != ingredients.len() {
                return None;
            }

            let mut ingredient_used = vec![false; ingredients.len()];
            'next_slot: for i in 0..inventory.size() {
                let slot = inventory.get_stack(i).await;
                let slot = slot.lock().await;

                if slot.is_empty() {
                    continue 'next_slot;
                }

                for i in 0..ingredients.len() {
                    if !ingredient_used[i] && ingredients[i].match_item(slot.item) {
                        ingredient_used[i] = true;
                        continue 'next_slot;
                    }
                }

                return None;
            }

            // TODO: Apply components
            Some(result)
        }
        CraftingRecipeTypes::CraftingTransmute {
            input,
            material,
            result,
            ..
        } => {
            if count != 2 {
                return None;
            }

            'item_stack: for i in 0..inventory.size() {
                let slot = inventory.get_stack(i).await;
                let slot = slot.lock().await;

                if slot.is_empty() {
                    continue 'item_stack;
                }

                if !material.match_item(slot.item) && !input.match_item(slot.item) {
                    return None;
                }
            }

            // TODO: Copy components
            Some(result)
        }
        CraftingRecipeTypes::CraftingDecoratedPot { .. } => {
            if count != 4 || inventory.get_width() != 3 || inventory.get_height() != 3 {
                return None;
            }

            for position in (1..=7).step_by(2) {
                let slot = inventory.get_stack(position).await;
                let slot = slot.lock().await;

                if slot.is_empty()
                    || !slot
                        .item
                        .has_tag(&tag::Item::MINECRAFT_DECORATED_POT_INGREDIENTS)
                {
                    return None;
                }
            }

            // TODO: Handle side textures
            Some(&RecipeResultStruct {
                id: "minecraft:decorated_pot",
                count: 1,
            })
        }
        CraftingRecipeTypes::CraftingSpecial => None,
    }
}


// CraftingMenu
pub struct CraftingTableScreenHandler {
    behaviour: ScreenHandlerBehaviour,
    crafting_inventory: Arc<dyn RecipeInputInventory>,
}

impl CraftingTableScreenHandler {
    pub async fn new(sync_id: u8, player_inventory: &Arc<PlayerInventory>) -> Self {
        let crafting_inventory: Arc<dyn RecipeInputInventory> =
            Arc::new(WorkstationInventory::new(3, 3));

        let mut crafting_table_handler = CraftingTableScreenHandler {
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Crafting)),
            crafting_inventory: crafting_inventory.clone(),
        };

        crafting_table_handler
            .add_recipe_slots(crafting_inventory)
            .await;

        // Add player inventory slots
        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        crafting_table_handler.add_player_slots(&player_inventory);

        crafting_table_handler
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

impl RecipeFinderScreenHandler for CraftingTableScreenHandler {}

impl ScreenHandler for CraftingTableScreenHandler {
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

impl CraftingScreenHandler<WorkstationInventory> for CraftingTableScreenHandler {}
