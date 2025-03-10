use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Network type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkType {
    /// Mainnet (production)
    Mainnet,
    /// Testnet (testing)
    Testnet,
    /// Devnet (development)
    Devnet,
}

impl NetworkType {
    /// Get the database representation of the network type
    pub fn as_db_str(&self) -> &'static str {
        match self {
            NetworkType::Mainnet => "mainnet",
            NetworkType::Testnet => "testnet",
            NetworkType::Devnet => "devnet",
        }
    }
    
    /// Parse a network type from a string with support for various aliases
    pub fn parse(s: &str) -> Option<Self> {
        // Get singleton instance of the alias map
        let aliases = network_aliases();
        
        // Convert input to lowercase for case-insensitive matching
        let lowered = s.to_lowercase();
        
        // Look up in the alias map
        aliases.get(&lowered).cloned()
    }
}

/// Get the network type alias map
fn network_aliases() -> &'static HashMap<String, NetworkType> {
    static ALIASES: OnceLock<HashMap<String, NetworkType>> = OnceLock::new();
    
    ALIASES.get_or_init(|| {
        let mut map = HashMap::new();
        
        // Mainnet aliases
        for alias in ["main", "mainnet", "mirana"] {
            map.insert(alias.to_string(), NetworkType::Mainnet);
            map.insert(alias.to_uppercase(), NetworkType::Mainnet);
        }
        
        // Testnet aliases
        for alias in ["test", "testnet", "pudge", "meepo"] {
            map.insert(alias.to_string(), NetworkType::Testnet);
            map.insert(alias.to_uppercase(), NetworkType::Testnet);
        }
        
        // Devnet aliases
        for alias in ["dev", "devnet", "local"] {
            map.insert(alias.to_string(), NetworkType::Devnet);
            map.insert(alias.to_uppercase(), NetworkType::Devnet);
        }
        
        map
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_network_parse() {
        // Test mainnet aliases
        assert_eq!(NetworkType::parse("main"), Some(NetworkType::Mainnet));
        assert_eq!(NetworkType::parse("MAIN"), Some(NetworkType::Mainnet));
        assert_eq!(NetworkType::parse("mainnet"), Some(NetworkType::Mainnet));
        assert_eq!(NetworkType::parse("MAINNET"), Some(NetworkType::Mainnet));
        assert_eq!(NetworkType::parse("mirana"), Some(NetworkType::Mainnet));
        assert_eq!(NetworkType::parse("MIRANA"), Some(NetworkType::Mainnet));
        
        // Test testnet aliases
        assert_eq!(NetworkType::parse("test"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::parse("TEST"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::parse("testnet"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::parse("TESTNET"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::parse("pudge"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::parse("PUDGE"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::parse("meepo"), Some(NetworkType::Testnet));
        assert_eq!(NetworkType::parse("MEEPO"), Some(NetworkType::Testnet));
        
        // Test devnet aliases
        assert_eq!(NetworkType::parse("dev"), Some(NetworkType::Devnet));
        assert_eq!(NetworkType::parse("DEV"), Some(NetworkType::Devnet));
        assert_eq!(NetworkType::parse("devnet"), Some(NetworkType::Devnet));
        assert_eq!(NetworkType::parse("DEVNET"), Some(NetworkType::Devnet));
        assert_eq!(NetworkType::parse("local"), Some(NetworkType::Devnet));
        assert_eq!(NetworkType::parse("LOCAL"), Some(NetworkType::Devnet));
        
        // Test invalid network type
        assert_eq!(NetworkType::parse("invalid"), None);
        assert_eq!(NetworkType::parse(""), None);
    }
} 