use crate::keywords::has_phrase;

pub const OTHER: &str = "other";

pub const MACRO_CATEGORIES: &[&str] = &[
    "fruit",
    "vegetables",
    "herbs_aromatics",
    "meat",
    "poultry",
    "fish_seafood",
    "cheese",
    "deli_cold_cuts",
    "dairy",
    "plant_dairy",
    "eggs",
    "bakery",
    "pasta_rice_grains",
    "baking_essentials",
    "canned_jars",
    "oils_vinegars_condiments",
    "spices_seasonings",
    "savory_snacks",
    "sweets_chocolate",
    "breakfast",
    "frozen",
    "ready_meals",
    "baby_food",
    "water",
    "soft_drinks",
    "juices",
    "coffee_tea",
    "alcohol",
    "household_cleaning",
    "personal_care",
    "paper_disposables",
    "other",
];

const DRINK_SLUGS: &[&str] = &["water", "soft_drinks", "juices", "coffee_tea", "alcohol"];

const RULES: &[(&str, &[&str])] = &[
    (
        "household_cleaning",
        &[
            "shopping bag",
            "plast shopping",
            "delivery fee",
            "service fee",
            "detergent",
            "omo",
            "comfort",
            "bleach",
            "clrx",
            "stain rem",
            "wash-up",
            "airfryer",
            "hmd pulse",
            "elfbar",
            "film",
            "fujifilm",
        ],
    ),
    (
        "personal_care",
        &[
            "toothbrush", "t/b", "t/brush", "toothpaste", "t/p", "colgate tb", "shampoo", "shmp",
            "deodorant", "rexona", "dove ro", "scholl", "wax", "trisa", "streax", "just for men",
            "hair color", "rstr", "odor", "swim short",
        ],
    ),
    (
        "paper_disposables",
        &["cling", "diamond cling", "toilet paper", "kitchen roll", "tissue"],
    ),
    (
        "alcohol",
        &[
            "heineken", "beer", "wine", "whisky", "whiskey", "vodka", "champagne", "prosecco",
            "cider", "gin tonic", "rum ", "spirit",
        ],
    ),
    (
        "water",
        &[
            "still water",
            "sparkling water",
            "mineral water",
            "masafi",
            "aquafina",
            "oasis blu",
            "san pel",
            "pel.",
            "perrier",
            "pellegrino",
            "acqua blu",
            "mai dubai",
        ],
    ),
    (
        "soft_drinks",
        &[
            "cola", "coca", "coke", "sprite", "fanta", "pepsi", "7up", "soda", "soft drink",
            "zero sugar", "energy drink", "red bull",
        ],
    ),
    ("juices", &["juice", "nectar", "smoothie"]),
    (
        "coffee_tea",
        &["nescafe", "coffee", "espresso", "cappuccino", "tea bag", "green tea", "black tea"],
    ),
    (
        "baby_food",
        &["baby food", "infant formula", "follow-on", "omogeneiz", "baby puree"],
    ),
    ("eggs", &["egg", "uova", "eggs"]),
    (
        "cheese",
        &[
            "cheese", "brie", "gorgonzola", "mascarpone", "mozzarella", "feta", "cheddar",
            "parmesan", "parmigiano", "ricotta", "camembert", "gruyere", "emmental", "halloumi",
            "cottage cheese", "cream cheese", "philadelphia", "ciliegine", "burrata",
        ],
    ),
    (
        "deli_cold_cuts",
        &[
            "salami", "prosciutto", "ham", "mortadella", "bresaola", "pepperoni", "cold cut",
            "deli meat", "turkey slice", "pastrami",
        ],
    ),
    (
        "fish_seafood",
        &[
            "salmon", "tuna", "shrimp", "prawn", "seafood", "fish", "cod", "sardine", "anchov",
            "calamari", "squid", "mussel", "crab", "lobster",
        ],
    ),
    ("poultry", &["chicken", "turkey", "duck", "poultry", "drumstick", "wing"]),
    (
        "meat",
        &[
            "beef", "steak", "stea", "lamb", "veal", "mince", "burger patty", "ribeye", "sirloin",
            "pork", "sausage", "merguez", "kebab meat",
        ],
    ),
    (
        "plant_dairy",
        &[
            "almond milk", "soy milk", "oat milk", "coconut drink", "plant milk", "vegan cheese",
            "vegan yogurt",
        ],
    ),
    (
        "dairy",
        &["milk", "yogurt", "yoghurt", "butter", "cream", "kefir", "labneh", "ghee"],
    ),
    (
        "herbs_aromatics",
        &["basil", "parsley", "mint", "coriander bunch", "dill", "rosemary", "thyme"],
    ),
    (
        "fruit",
        &[
            "apple", "banana", "orange", "lemon", "lime", "grape", "berry", "mango", "melon",
            "watermelon", "pineapple", "avocado", "pear", "peach", "plum", "strawberr", "blueberr",
            "raspberr", "kiwi", "dates", "fig",
        ],
    ),
    (
        "vegetables",
        &[
            "tomato", "potato", "onion", "carrot", "cucumber", "lettuce", "salad", "pepper",
            "broccoli", "spinach", "cabbage", "eggplant", "zucchini", "cauliflower", "mushroom",
            "garlic", "ginger root", "celery",
        ],
    ),
    ("frozen", &["frozen", "surgel", "ice cream", "gelato"]),
    (
        "ready_meals",
        &[
            "ready meal", "meal kit", "big mac", "mcnugget", "shawarma", "burrito", "sandwich",
            "pizza slice", "lasagne", "biryani",
        ],
    ),
    (
        "bakery",
        &[
            "bread", "pita", "croissant", "bagel", "muffin", "brioche", "focaccia", "crackers",
            "water cracker", "flatbread", "tortilla wrap",
        ],
    ),
    (
        "pasta_rice_grains",
        &[
            "pasta", "spaghetti", "penne", "fusilli", "rice", "basmati", "quinoa", "couscous",
            "bulgur", "noodle", "vermicelli",
        ],
    ),
    (
        "baking_essentials",
        &["flour", "baking powder", "yeast", "icing sugar", "caster sugar", "cocoa powder"],
    ),
    (
        "canned_jars",
        &[
            "canned", "tinned", "in brine", "in oil", "hummus", "tahini", "jam", "marmalade",
            "peanut butter", "nutella",
        ],
    ),
    (
        "oils_vinegars_condiments",
        &[
            "olive oil", "sunflower oil", "vegetable oil", "vinegar", "ketchup", "mayonnaise",
            "mustard", "soy sauce", "pasta sauce", "tomato sauce", "salsa", "bbq sauce",
            "hot sauce", "dressing",
        ],
    ),
    (
        "spices_seasonings",
        &[
            "spice", "curry", "cumin", "paprika", "turmeric", "peppercorn", "zaatar", "stock cube",
            "bouillon", "seasoning", "salt", "sea salt",
        ],
    ),
    (
        "savory_snacks",
        &["chips", "crisps", "pringles", "popcorn", "nachos", "pretzel", "snack mix"],
    ),
    (
        "sweets_chocolate",
        &[
            "chocolate", "candy", "biscuit", "cookie", "wafer", "lindt", "kinder", "haribo",
            "gummy", "cake", "brownie", "donut", "doughnut",
        ],
    ),
    (
        "breakfast",
        &["cereal", "muesli", "granola", "oats", "porridge", "corn flakes", "honey"],
    ),
];

const EDIBLE_OVERRIDE: &[&str] = &[
    "vinegar",
    "milk chocolate",
    "chocolate bar",
    "chocolate cake",
    "coffee cake",
    "tea biscuit",
    "wine gum",
    "wine vinegar",
    "coconut milk",
    "baking soda",
    "water cracker",
    "rose water",
    "beer batter",
];

pub fn is_valid_macro(slug: &str) -> bool {
    MACRO_CATEGORIES.contains(&slug.trim())
}

pub fn normalize_macro(slug: &str) -> String {
    let c = slug.trim();
    if is_valid_macro(c) {
        c.to_string()
    } else {
        String::new()
    }
}

pub fn macro_label(slug: &str) -> &'static str {
    match normalize_macro(slug).as_str() {
        "fruit" => "Fruit",
        "vegetables" => "Vegetables",
        "herbs_aromatics" => "Herbs & aromatics",
        "meat" => "Meat",
        "poultry" => "Poultry",
        "fish_seafood" => "Fish & seafood",
        "cheese" => "Cheese",
        "deli_cold_cuts" => "Deli & cold cuts",
        "dairy" => "Dairy",
        "plant_dairy" => "Plant dairy",
        "eggs" => "Eggs",
        "bakery" => "Bakery",
        "pasta_rice_grains" => "Pasta, rice & grains",
        "baking_essentials" => "Baking essentials",
        "canned_jars" => "Canned & jars",
        "oils_vinegars_condiments" => "Oils, vinegars & condiments",
        "spices_seasonings" => "Spices & seasonings",
        "savory_snacks" => "Savory snacks",
        "sweets_chocolate" => "Sweets & chocolate",
        "breakfast" => "Breakfast",
        "frozen" => "Frozen",
        "ready_meals" => "Ready meals",
        "baby_food" => "Baby food",
        "water" => "Water",
        "soft_drinks" => "Soft drinks",
        "juices" => "Juices",
        "coffee_tea" => "Coffee & tea",
        "alcohol" => "Alcohol",
        "household_cleaning" => "Household & cleaning",
        "personal_care" => "Personal care",
        "paper_disposables" => "Paper & disposables",
        "other" => "Other",
        _ => "",
    }
}

pub fn classify_macro(name: &str, extra: &str) -> String {
    let blob = format!("{} {}", name, extra).trim().to_string();
    if blob.is_empty() {
        return OTHER.into();
    }
    let edible = EDIBLE_OVERRIDE.iter().any(|p| has_phrase(&blob, p));
    for (slug, words) in RULES {
        if !words.iter().any(|w| has_phrase(&blob, w)) {
            continue;
        }
        if edible && DRINK_SLUGS.contains(slug) {
            continue;
        }
        if *slug == "dairy" && has_phrase(&blob, "chocolate") {
            continue;
        }
        return (*slug).into();
    }
    OTHER.into()
}

pub fn classify_macro_from_open_facts(category: &str) -> String {
    let blob = category.to_ascii_lowercase();
    if blob.is_empty() {
        return String::new();
    }
    const HINTS: &[(&str, &[&str])] = &[
        ("cheese", &["cheese", "fromage"]),
        ("fish_seafood", &["fish", "seafood", "tuna", "salmon"]),
        ("meat", &["meat", "beef", "pork", "lamb"]),
        ("poultry", &["poultry", "chicken"]),
        ("dairy", &["dairy", "yogurt", "milk"]),
        ("sweets_chocolate", &["chocolate", "confection", "candy", "biscuit"]),
        ("soft_drinks", &["soft drink", "soda", "cola"]),
        ("juices", &["juice"]),
        ("water", &["water", "mineral"]),
        ("alcohol", &["beer", "wine", "spirit"]),
        ("bakery", &["bread", "bakery"]),
        ("pasta_rice_grains", &["pasta", "rice", "cereal grain"]),
        ("oils_vinegars_condiments", &["oil", "vinegar", "sauce"]),
        ("savory_snacks", &["snack", "crisp", "chip"]),
        ("personal_care", &["shampoo", "cosmetic", "toothpaste"]),
        ("household_cleaning", &["detergent", "cleaning"]),
    ];
    for (slug, words) in HINTS {
        if words.iter().any(|w| blob.contains(w)) {
            return (*slug).into();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheese_and_milk_land_in_different_shelves() {
        assert_eq!(classify_macro("Cheddar block", ""), "cheese");
        assert_eq!(classify_macro("Almarai Full Fat Milk 2L", ""), "dairy");
        assert_eq!(classify_macro("S.Pellegrino Sparkling Natural Mineral Water", ""), "water");
        assert_eq!(classify_macro("milk chocolate bar", ""), "sweets_chocolate");
    }
}
