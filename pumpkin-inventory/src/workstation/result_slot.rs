use std::any::Any;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::recipes::{RecipeFinderScreenHandler, RecipeInputInventory};
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



/// CraftingResultSlot.java
///
/// Note: This implementation is different from the original Minecraft code.
/// Particularly, it does not have a 'result' inventory, we directly store it in the slot.
/// This slot should be never modified outside. any modifications to it make change in its input.
pub struct ResultSlot {
    pub inventory: Arc<dyn RecipeInputInventory>,
    pub id: AtomicU8,
    pub result: Arc<Mutex<ItemStack>>,
    recipe_cache: AtomicCell<Option<&'static CraftingRecipeTypes>>,
}

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

impl ResultSlot {
    fn stat_crafted(&self, _crafted_amount: u8, _player: &dyn InventoryPlayer) {}

    pub fn new(inventory: Arc<dyn RecipeInputInventory>) -> Self {
        Self {
            inventory,
            id: AtomicU8::new(0),
            result: Arc::new(Mutex::new(ItemStack::EMPTY.clone())),
            recipe_cache: AtomicCell::new(None),
        }
    }

    /// Matches the recipe in the crafting inventory and returns the result.
    ///
    /// If no recipe matches, returns `None`.
    async fn match_recipe(&self) -> Option<(&RecipeResultStruct, &'static CraftingRecipeTypes)> {
        let mut count: usize = 0;
        let inventory_width = self.inventory.get_width();
        let mut top_x = 9;
        let mut top_y = 9;
        let mut bottom_x = 0;
        let mut bottom_y = 0;
        for i in 0..self.inventory.size() {
            let x = i % inventory_width;
            let y = i / inventory_width;

            let slot = self.inventory.get_stack(i).await;
            let slot = slot.lock().await;
            if !slot.is_empty() {
                top_x = top_x.min(x);
                top_y = top_y.min(y);
                bottom_x = bottom_x.max(x);
                bottom_y = bottom_y.max(y);
                count += 1;
            }
        }

        if count == 0 {
            return None;
        }
        let input_width = bottom_x + 1 - top_x;
        let input_height = bottom_y + 1 - top_y;

        if let Some(cached_recipe) = self.recipe_cache.load() {
            if let Some(result) = recipe_matches(
                cached_recipe,
                input_height,
                input_width,
                top_x,
                top_y,
                count,
                &*self.inventory,
            )
            .await
            {
                return Some((result, cached_recipe));
            }
        }

        for recipe in RECIPES_CRAFTING {
            if let Some(result) = recipe_matches(
                recipe,
                input_height,
                input_width,
                top_x,
                top_y,
                count,
                &*self.inventory,
            )
            .await
            {
                self.recipe_cache.store(Some(recipe));
                return Some((result, recipe));
            }
        }

        None
    }

    async fn refill_output(&self) -> ItemStack {
        let result = self
            .match_recipe()
            .await
            .map(|x| ItemStack::from(x.0))
            .unwrap_or(ItemStack::EMPTY.clone());
        *self.result.lock().await = result.clone();
        result
    }
}

impl Slot for ResultSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.inventory.clone()
    }

    fn get_index(&self) -> usize {
        999 // this slot does not belong to any inventory
    }

    fn set_id(&self, id: usize) {
        self.id.store(id as u8, Ordering::Relaxed);
    }

    fn on_quick_move_crafted(
        &self,
        _stack: ItemStack,
        _stack_prev: ItemStack,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // refill the result slot with the recipe result
            self.refill_output().await;
        })
    }

    fn on_take_item<'a>(
        &'a self,
        player: &'a dyn InventoryPlayer,
        stack: &'a ItemStack,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            for i in 0..self.inventory.size() {
                let slot = self.inventory.get_stack(i).await;
                let mut stack = slot.lock().await;
                if !stack.is_empty() {
                    //TODO: Handle remaining items.
                    stack.item_count -= 1;
                }
            }
            self.stat_crafted(stack.item_count, player);
            self.mark_dirty().await;
        })
    }

    fn can_insert(&self, _stack: &ItemStack) -> BoxFuture<'_, bool> {
        Box::pin(async move { false })
    }

    fn get_stack(&self) -> BoxFuture<'_, Arc<Mutex<ItemStack>>> {
        Box::pin(async move { self.result.clone() })
    }

    fn get_cloned_stack(&self) -> BoxFuture<'_, ItemStack> {
        Box::pin(async move { self.result.lock().await.clone() })
    }

    fn has_stack(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move { !self.result.lock().await.is_empty() })
    }

    fn set_stack(&self, _stack: ItemStack) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.refill_output().await;
        })
    }

    fn set_stack_prev(&self, _stack: ItemStack, _previous_stack: ItemStack) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.refill_output().await;
        })
    }

    fn mark_dirty(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.inventory.mark_dirty();
        })
    }

    fn get_max_item_count(&self) -> BoxFuture<'_, u8> {
        Box::pin(async move {
            let mut count = u8::MAX;
            for i in 0..self.inventory.size() {
                let slot = self.inventory.get_stack(i).await;
                let slot = slot.lock().await;
                if !slot.is_empty() {
                    count = count.min(slot.item_count);
                }
            }
            count
        })
    }

    fn take_stack(&self, _amount: u8) -> BoxFuture<'_, ItemStack> {
        Box::pin(async move {
            if self.has_stack().await {
                let stack = self.result.lock().await;
                // Vanilla: net.minecraft.world.inventory.ResultContainer#removeItem
                // Regardless of the amount, we always return the full stack
                stack.clone()
            } else {
                ItemStack::EMPTY.clone()
            }
        })
    }
}

impl ScreenHandlerListener for ResultSlot {
    fn on_slot_update<'a>(
        &'a self,
        screen_handler: &'a ScreenHandlerBehaviour,
        slot: u8,
        _stack: ItemStack,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if (0..=(self.inventory.get_width() * self.inventory.get_height()))
                .contains(&(slot as usize))
            {
                let result = self.refill_output().await;

                let next_revision = screen_handler.next_revision();
                if let Some(sync_handler) = screen_handler.sync_handler.as_ref() {
                    sync_handler
                        .update_slot(screen_handler, 0, &result, next_revision)
                        .await;
                }
            }
        })
    }
}
