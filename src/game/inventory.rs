//! Bounded slot inventory and deterministic atomic recipe crafting.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub key: String,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub key: String,
    pub inputs: Vec<ItemStack>,
    pub output: ItemStack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InventoryError {
    InvalidCapacity,
    InvalidItemKey(String),
    InvalidRecipe,
    MissingIngredients(String),
    CapacityExceeded,
    AllocationFailed,
}

impl std::fmt::Display for InventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InventoryError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    max_stack: u32,
    slots: Vec<Option<ItemStack>>,
}

impl Inventory {
    pub fn try_new(slot_count: usize, max_stack: u32) -> Result<Self, InventoryError> {
        if slot_count == 0 || slot_count > 256 || max_stack == 0 || max_stack > 1_000_000 {
            return Err(InventoryError::InvalidCapacity);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(slot_count)
            .map_err(|_| InventoryError::AllocationFailed)?;
        slots.resize_with(slot_count, || None);
        Ok(Self { max_stack, slots })
    }

    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    pub(crate) fn from_snapshot(
        max_stack: u32,
        slots: Vec<Option<ItemStack>>,
    ) -> Result<Self, InventoryError> {
        if slots.is_empty() || slots.len() > 256 || max_stack == 0 || max_stack > 1_000_000 {
            return Err(InventoryError::InvalidCapacity);
        }
        for stack in slots.iter().flatten() {
            validate_stack(stack, max_stack)?;
        }
        Ok(Self { max_stack, slots })
    }

    pub(crate) fn max_stack(&self) -> u32 {
        self.max_stack
    }

    pub fn staged_add(&self, stack: ItemStack) -> Result<Self, InventoryError> {
        validate_stack(&stack, self.max_stack)?;
        if self.capacity_after_removal(&stack.key, &[]) < u64::from(stack.count) {
            return Err(InventoryError::CapacityExceeded);
        }
        let mut staged = self.try_clone()?;
        staged.add_unchecked(stack)?;
        Ok(staged)
    }

    pub fn count(&self, key: &str) -> u64 {
        self.slots
            .iter()
            .filter_map(Option::as_ref)
            .filter(|stack| stack.key == key)
            .map(|stack| u64::from(stack.count))
            .sum()
    }

    pub fn add(&mut self, stack: ItemStack) -> Result<(), InventoryError> {
        validate_stack(&stack, self.max_stack)?;
        if self.capacity_after_removal(&stack.key, &[]) < u64::from(stack.count) {
            return Err(InventoryError::CapacityExceeded);
        }
        let mut staged = self.try_clone()?;
        staged.add_unchecked(stack)?;
        *self = staged;
        Ok(())
    }

    pub fn remove(&mut self, key: &str, count: u32) -> Result<(), InventoryError> {
        validate_key(key)?;
        if count == 0 {
            return Err(InventoryError::InvalidRecipe);
        }
        if self.count(key) < u64::from(count) {
            return Err(InventoryError::MissingIngredients(key.to_string()));
        }
        let mut remaining = count;
        for slot in &mut self.slots {
            if let Some(stack) = slot
                && stack.key == key
                && remaining > 0
            {
                let take = remaining.min(stack.count);
                stack.count -= take;
                remaining -= take;
                if stack.count == 0 {
                    *slot = None;
                }
            }
        }
        Ok(())
    }

    pub fn craft(&mut self, recipe: &Recipe) -> Result<(), InventoryError> {
        recipe.validate(self.max_stack)?;
        let requirements = aggregate_inputs(&recipe.inputs)?;
        let mut staged = self.try_clone()?;
        let mut removals = Vec::new();
        removals
            .try_reserve_exact(staged.slots.len().saturating_mul(requirements.len()))
            .map_err(|_| InventoryError::AllocationFailed)?;

        for requirement in &requirements {
            let mut remaining = requirement.count;
            for (index, slot) in staged.slots.iter().enumerate() {
                if let Some(stack) = slot
                    && stack.key == requirement.key
                    && remaining > 0
                {
                    let take = remaining.min(stack.count);
                    removals.push((index, take));
                    remaining -= take;
                }
            }
            if remaining > 0 {
                return Err(InventoryError::MissingIngredients(requirement.key.clone()));
            }
        }

        if staged.capacity_after_removal(&recipe.output.key, &removals)
            < u64::from(recipe.output.count)
        {
            return Err(InventoryError::CapacityExceeded);
        }

        for (index, count) in removals {
            let stack = staged.slots[index]
                .as_mut()
                .expect("preflighted inventory slot");
            stack.count -= count;
            if stack.count == 0 {
                staged.slots[index] = None;
            }
        }
        staged.add_unchecked(try_clone_stack(&recipe.output)?)?;
        *self = staged;
        Ok(())
    }

    fn try_clone(&self) -> Result<Self, InventoryError> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(self.slots.len())
            .map_err(|_| InventoryError::AllocationFailed)?;
        for slot in &self.slots {
            slots.push(match slot {
                Some(stack) => Some(try_clone_stack(stack)?),
                None => None,
            });
        }
        Ok(Self {
            max_stack: self.max_stack,
            slots,
        })
    }

    fn capacity_after_removal(&self, output_key: &str, removals: &[(usize, u32)]) -> u64 {
        let mut capacity = 0_u64;
        for (index, slot) in self.slots.iter().enumerate() {
            let removed = removals
                .iter()
                .filter(|(slot_index, _)| *slot_index == index)
                .map(|(_, count)| u64::from(*count))
                .sum::<u64>();
            match slot {
                Some(stack) if stack.key == output_key => {
                    capacity += u64::from(self.max_stack - stack.count) + removed;
                }
                Some(stack) if removed >= u64::from(stack.count) => {
                    capacity += u64::from(self.max_stack);
                }
                Some(_) => {}
                None => capacity += u64::from(self.max_stack),
            }
        }
        capacity
    }

    fn add_unchecked(&mut self, mut stack: ItemStack) -> Result<(), InventoryError> {
        for slot in &mut self.slots {
            if let Some(existing) = slot
                && existing.key == stack.key
                && existing.count < self.max_stack
            {
                let added = stack.count.min(self.max_stack - existing.count);
                existing.count += added;
                stack.count -= added;
                if stack.count == 0 {
                    return Ok(());
                }
            }
        }
        for slot in &mut self.slots {
            if slot.is_none() {
                let added = stack.count.min(self.max_stack);
                let mut key = String::new();
                key.try_reserve_exact(stack.key.len())
                    .map_err(|_| InventoryError::AllocationFailed)?;
                key.push_str(&stack.key);
                *slot = Some(ItemStack { key, count: added });
                stack.count -= added;
                if stack.count == 0 {
                    return Ok(());
                }
            }
        }
        if stack.count > 0 {
            Err(InventoryError::CapacityExceeded)
        } else {
            Ok(())
        }
    }
}

impl Recipe {
    pub fn validate(&self, max_stack: u32) -> Result<(), InventoryError> {
        validate_key(&self.key)?;
        validate_stack(&self.output, max_stack)?;
        if self.inputs.is_empty() || self.inputs.len() > 64 {
            return Err(InventoryError::InvalidRecipe);
        }
        for input in &self.inputs {
            validate_stack(input, max_stack)?;
        }
        Ok(())
    }
}

fn aggregate_inputs(inputs: &[ItemStack]) -> Result<Vec<ItemStack>, InventoryError> {
    let mut aggregated: Vec<ItemStack> = Vec::new();
    aggregated
        .try_reserve_exact(inputs.len())
        .map_err(|_| InventoryError::AllocationFailed)?;
    for input in inputs {
        if let Some(existing) = aggregated.iter_mut().find(|entry| entry.key == input.key) {
            existing.count = existing
                .count
                .checked_add(input.count)
                .ok_or(InventoryError::InvalidRecipe)?;
        } else {
            aggregated.push(try_clone_stack(input)?);
        }
    }
    Ok(aggregated)
}

fn try_clone_stack(stack: &ItemStack) -> Result<ItemStack, InventoryError> {
    let mut key = String::new();
    key.try_reserve_exact(stack.key.len())
        .map_err(|_| InventoryError::AllocationFailed)?;
    key.push_str(&stack.key);
    Ok(ItemStack {
        key,
        count: stack.count,
    })
}

fn validate_stack(stack: &ItemStack, max_stack: u32) -> Result<(), InventoryError> {
    validate_key(&stack.key)?;
    if stack.count == 0 || stack.count > max_stack {
        return Err(InventoryError::InvalidRecipe);
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), InventoryError> {
    let invalid = || InventoryError::InvalidItemKey(key.to_string());
    let mut parts = key.split(':');
    let (Some(namespace), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let valid = |part: &str| {
        !part.is_empty()
            && part.len() <= 64
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
    };
    if valid(namespace) && valid(name) {
        Ok(())
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(key: &str, count: u32) -> ItemStack {
        ItemStack {
            key: key.into(),
            count,
        }
    }

    #[test]
    fn crafting_consumes_inputs_and_adds_output() {
        let mut inventory = Inventory::try_new(4, 99).unwrap();
        inventory.add(stack("sample:ore", 3)).unwrap();
        let recipe = Recipe {
            key: "sample:ingot_recipe".into(),
            inputs: vec![stack("sample:ore", 2)],
            output: stack("sample:ingot", 1),
        };
        inventory.craft(&recipe).unwrap();
        assert_eq!(inventory.slots()[0], Some(stack("sample:ore", 1)));
        assert_eq!(inventory.slots()[1], Some(stack("sample:ingot", 1)));
    }

    #[test]
    fn missing_input_or_output_capacity_leaves_inventory_unchanged() {
        let mut inventory = Inventory::try_new(2, 3).unwrap();
        inventory.add(stack("sample:ore", 2)).unwrap();
        inventory.add(stack("sample:stone", 3)).unwrap();
        let before = inventory.clone();
        let recipe = Recipe {
            key: "sample:bar_recipe".into(),
            inputs: vec![stack("sample:ore", 1)],
            output: stack("sample:bar", 1),
        };
        assert_eq!(
            inventory.craft(&recipe),
            Err(InventoryError::CapacityExceeded)
        );
        assert_eq!(inventory, before);
    }

    #[test]
    fn duplicate_inputs_aggregate_before_validation_and_commit() {
        let mut inventory = Inventory::try_new(2, 10).unwrap();
        inventory.add(stack("sample:ore", 4)).unwrap();
        let recipe = Recipe {
            key: "sample:double_recipe".into(),
            inputs: vec![stack("sample:ore", 1), stack("sample:ore", 2)],
            output: stack("sample:bar", 1),
        };
        inventory.craft(&recipe).unwrap();
        assert_eq!(inventory.slots()[0], Some(stack("sample:ore", 1)));
    }

    #[test]
    fn remove_is_all_or_nothing_when_items_are_missing() {
        let mut inventory = Inventory::try_new(2, 10).unwrap();
        inventory.add(stack("sample:ore", 4)).unwrap();
        let before = inventory.clone();
        assert_eq!(
            inventory.remove("sample:ore", 5),
            Err(InventoryError::MissingIngredients("sample:ore".into()))
        );
        assert_eq!(inventory, before);
    }
}
