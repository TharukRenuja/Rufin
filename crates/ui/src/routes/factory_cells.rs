use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::ObjectType;

pub(crate) struct FactoryCells<T> {
    cells: Rc<RefCell<HashMap<usize, T>>>,
}

impl<T> FactoryCells<T> {
    pub(crate) fn new() -> Self {
        Self {
            cells: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub(crate) fn insert(&self, item: &gtk::ListItem, cell: T) {
        self.cells.borrow_mut().insert(item_key(item), cell);
    }

    pub(crate) fn remove(&self, item: &gtk::ListItem) {
        self.cells.borrow_mut().remove(&item_key(item));
    }
}

impl<T: Clone> FactoryCells<T> {
    pub(crate) fn get(&self, item: &gtk::ListItem) -> Option<T> {
        self.cells.borrow().get(&item_key(item)).cloned()
    }
}

impl<T> Clone for FactoryCells<T> {
    fn clone(&self) -> Self {
        Self {
            cells: Rc::clone(&self.cells),
        }
    }
}

fn item_key(item: &gtk::ListItem) -> usize {
    item.as_ptr() as usize
}
