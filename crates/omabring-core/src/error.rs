use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error")]
    Db(#[from] rusqlite::Error),
    #[error("invalid email")]
    InvalidEmail,
    #[error("password must be at least 8 characters")]
    WeakPassword,
    #[error("an account with that email already exists")]
    EmailTaken,
    #[error("incorrect email or password")]
    BadCredentials,
    #[error("sign in required")]
    Unauthorized,
    #[error("that store account is not linked")]
    StoreNotLinked,
    #[error("the cart is empty")]
    EmptyCart,
    #[error("invalid receipt: {0}")]
    InvalidInvoice(String),
    #[error("could not protect stored data")]
    Crypto,
    #[error("{error}: {description}")]
    Oauth {
        error: &'static str,
        description: String,
    },
    #[error("{0}")]
    Message(String),
    #[error("io error")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn oauth(error: &'static str, description: impl Into<String>) -> Self {
        Self::Oauth {
            error,
            description: description.into(),
        }
    }
}
