use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Deserialize, Serialize, Default)]
pub struct Cache {
    pub accounts: HashMap<String, Credential>,
    pub scopes: HashSet<String>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Credential {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(skip)]
    pub state: CredentialState,
}

#[derive(Default, Copy, Clone, PartialEq, Eq)]
pub enum CredentialState {
    #[default]
    Cached,
    Expired,
    Valid,
}

/// キャッシュの読み書きを行います。トークンなどの情報は有効であるとは限らないので、別途検証する必要があります。
pub struct CacheManager {
    cache_path: PathBuf,
}

impl CacheManager {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, AppError> {
        Ok(Self {
            cache_path: path.as_ref().to_owned(),
        })
    }

    pub fn load(&self) -> Result<Option<Cache>, AppError> {
        let mut file = match File::open(&self.cache_path) {
            Ok(file) => file,
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(x) => return Err(x).map_err(AppError::Io),
        };
        let mut s = String::new();
        file.read_to_string(&mut s)?;
        let content: Cache = serde_json::from_str(&s).map_err(AppError::CacheParse)?;

        Ok(Some(content))
    }

    pub fn save(
        &self,
        scopes: HashSet<String>,
        credentials: HashMap<String, Credential>,
    ) -> Result<(), AppError> {
        let content = Cache {
            scopes,
            accounts: credentials.into_iter().collect(),
        };
        let mut file = File::create(&self.cache_path)?;
        file.write_all(serde_json::to_string(&content).unwrap().as_bytes())?;
        Ok(())
    }
}
