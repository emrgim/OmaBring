use crate::keywords::has_any;

const DRINK: &[&str] = &[
    "cola", "coca", "coke", "sprite", "fanta", "pepsi", "7up", "water", "sparkling",
    "still water", "oasis", "perrier", "san pel", "pel.", "masafi", "aquafina", "acqua",
    "mai dubai", "juice", "drink", "beverage", "soda", "milk", "latte", "nescafe", "coffee",
    "tea", "zero sugar", "soft drink", "beer", "wine", "whisky", "whiskey", "vodka", "gin",
    "rum", "champagne", "prosecco", "cider", "heineken",
];

const EDIBLE_OVERRIDE: &[&str] = &[
    "vinegar", "olive oil", "milk chocolate", "chocolate bar", "chocolate cake", "coffee cake",
    "tea biscuit", "tea cookie", "wine gum", "wine vinegar", "coconut milk", "baking soda",
    "water cracker", "rose water",
];

const NONEDIBLE: &[&str] = &[
    "toothbrush", "t/b", "t/brush", "toothpaste", "t/p", "colgate tb", "shampoo", "shmp",
    "deodorant", "rexona", "dove ro", "comfort", "omo", "detergent", "stain rem", "bleach",
    "clrx", "film", "fujifilm", "mini film", "hmd", "pulse pro", "elfbar", "airfryer", "swim",
    "swim short", "odor", "scholl", "wax", "trisa", "prestiges", "streax", "shopping bag",
    "plast shopping", "frozen bag", "ppw bag", "wash-up", "cling", "just for men", "hair color",
    "rstr", "diamond cling",
];

const NOT_DRINK: &[&str] = &["toothpaste", "toothbrush", "colgate tb", "t/p", "t/b", "t/brush"];

pub fn normalize_dept(dept: &str) -> String {
    match dept.trim() {
        "Edible" | "Food" => "Edible".into(),
        "Drinks" => "Drinks".into(),
        _ => String::new(),
    }
}

pub fn classify_dept(name: &str, extra: &str) -> String {
    let blob = format!("{name} {extra}");
    if has_any(&blob, EDIBLE_OVERRIDE) {
        return "Edible".into();
    }
    if has_any(&blob, DRINK) {
        if has_any(&blob, NOT_DRINK) {
            return String::new();
        }
        return "Drinks".into();
    }
    if has_any(&blob, NONEDIBLE) {
        return String::new();
    }
    "Edible".into()
}

pub fn matches_dept(dept: &str, name: &str, extra: &str) -> bool {
    let want = normalize_dept(dept);
    if want.is_empty() {
        return true;
    }
    classify_dept(name, extra) == want
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tea_is_a_drink_and_steak_is_not() {
        assert_eq!(classify_dept("green tea", ""), "Drinks");
        assert_eq!(classify_dept("ribeye steak", ""), "Edible");
        assert_eq!(classify_dept("virgin olive oil", ""), "Edible");
        assert_eq!(classify_dept("Colgate toothpaste", ""), "");
        assert_eq!(classify_dept("Heineken cans", ""), "Drinks");
    }
}
