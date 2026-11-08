//! The one place that knows which Kaspa network a vault lives on. `main` builds a `Network` from
//! `--network testnet|mainnet` (default testnet) and carries it in `Ctx`; every address, node id,
//! fee floor and clock rate is read from here and nowhere else. keys.json and state.json record
//! the network they were made for, and a folder refuses to run under any other flag.
use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::network::{NetworkId, NetworkType};
use kaspa_consensus_core::tx::ScriptPublicKey;
use kaspa_txscript::{extract_script_pub_key_address, pay_to_address_script};

/// Relay minimum on testnet-10 as observed: 100 sompi per gram of mass.
pub const TESTNET_FEE_FLOOR: u64 = 100;
/// Mainnet floor until change list item 5 measures it with get_fee_estimate at connect; the same
/// relay minimum applies on both networks today.
pub const MAINNET_FEE_FLOOR: u64 = 100;
/// Both networks advance the DAA score about ten per second (item 5 measures the real rate).
pub const DAA_PER_SECOND: f64 = 10.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    /// The word recorded in files and shown to people: "testnet" or "mainnet".
    pub name: &'static str,
    /// What a node must report (`get_server_info`) and what the resolver is asked for.
    pub id: NetworkId,
    /// The address prefix every address of this vault must carry.
    pub prefix: Prefix,
    /// The dials preset this network runs with (contract.rs owns the numbers).
    pub dials: &'static str,
    /// Fee rate floor, sompi per gram of mass; `connect` only raises it.
    pub fee_floor: u64,
    /// DAA units per second of wall time.
    pub daa_per_sec: f64,
}

pub const TESTNET: &str = "testnet";
pub const MAINNET: &str = "mainnet";

impl Network {
    pub fn testnet() -> Network {
        Network { name: TESTNET, id: NetworkId::with_suffix(NetworkType::Testnet, 10), prefix: Prefix::Testnet, dials: "testnet", fee_floor: TESTNET_FEE_FLOOR, daa_per_sec: DAA_PER_SECOND }
    }
    pub fn mainnet() -> Network {
        Network { name: MAINNET, id: NetworkId::new(NetworkType::Mainnet), prefix: Prefix::Mainnet, dials: "mainnet", fee_floor: MAINNET_FEE_FLOOR, daa_per_sec: DAA_PER_SECOND }
    }
    /// The `--network` flag.
    pub fn from_flag(s: &str) -> Result<Network, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            TESTNET | "testnet-10" | "tn10" => Ok(Network::testnet()),
            MAINNET => Ok(Network::mainnet()),
            other => Err(format!("`{other}` is not a network this program knows. Use --network testnet or --network mainnet.")),
        }
    }
    /// The network a file was made for, from the word it recorded. Files from before the word was
    /// recorded (empty) were all made on testnet; `check_recorded` refuses them on mainnet.
    pub fn of_record(recorded: &str) -> Network {
        if recorded == MAINNET { Network::mainnet() } else { Network::testnet() }
    }
    pub fn is_mainnet(&self) -> bool {
        self.name == MAINNET
    }
    /// The resolver asks for this network when no node url is pinned.
    pub fn resolver_id(&self) -> NetworkId {
        self.id
    }

    /// A folder file (keys.json, state.json, a signing request) made for `recorded` may run here
    /// only when it names this network. No record at all passes on testnet (older files) and is
    /// refused on mainnet, where every file must say so.
    pub fn check_recorded(&self, recorded: &str, what: &str) -> Result<(), String> {
        if recorded == self.name {
            return Ok(());
        }
        if recorded.is_empty() {
            if self.is_mainnet() {
                return Err(format!("{what} does not say which network it was made for, and mainnet needs that. Make the vault again with --network mainnet, or run this folder with --network testnet."));
            }
            return Ok(());
        }
        Err(format!("{what} was made for the {recorded} network; this program was started for {}. Run it with --network {recorded}, or use the folder made for {}.", self.name, self.name))
    }
    /// The node must be on this network; one that says otherwise is lying or misconfigured.
    pub fn check_node(&self, reported: &NetworkId, node: &str) -> Result<(), String> {
        if *reported == self.id {
            Ok(())
        } else {
            Err(format!("the node at {node} says it is on {reported}, not {}. It is lying or set up wrong; use a node on {} or start this program with the matching --network.", self.id, self.id))
        }
    }

    /// The one address check: a readable address carrying this network's prefix, or a refusal.
    pub fn address(&self, s: &str) -> Result<Address, String> {
        let a = Address::try_from(s.trim()).map_err(|_| format!("`{s}` is not a Kaspa address. Check it and try again; a {} address starts with {}:", self.name, self.prefix))?;
        if a.prefix != self.prefix {
            return Err(format!("`{s}` is a {} address; this vault is on {} and only sends to addresses that start with {}:", a.prefix, self.name, self.prefix));
        }
        Ok(a)
    }
    /// The script an address pays to, after `address`.
    pub fn try_to_spk(&self, s: &str) -> Result<ScriptPublicKey, String> {
        Ok(pay_to_address_script(&self.address(s)?))
    }
    /// `try_to_spk` for addresses this program wrote itself (keys.json); a bad one is a broken file.
    pub fn to_spk(&self, s: &str) -> ScriptPublicKey {
        self.try_to_spk(s).unwrap_or_else(|e| panic!("{e}"))
    }
    /// The address a script pays, rendered with this network's prefix; None for a script that is not one.
    pub fn address_of(&self, spk: &ScriptPublicKey) -> Option<Address> {
        extract_script_pub_key_address(spk, self.prefix).ok()
    }
    /// Pay-to-pubkey address of an x-only key.
    pub fn p2pk(&self, x: &[u8]) -> Address {
        Address::new(self.prefix, Version::PubKey, x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_value_from_flag() {
        let t = Network::from_flag("testnet").unwrap();
        let m = Network::from_flag("MainNet").unwrap();
        assert_eq!((t.name, t.prefix, t.id.to_string().as_str()), ("testnet", Prefix::Testnet, "testnet-10"));
        assert_eq!((m.name, m.prefix, m.id.to_string().as_str()), ("mainnet", Prefix::Mainnet, "mainnet"));
        assert!(Network::from_flag("devnet").unwrap_err().contains("--network testnet or --network mainnet"));
        assert_eq!(Network::of_record(""), t);
        assert_eq!(Network::of_record("mainnet"), m);
        assert!(t.check_node(&m.id, "ws://x").unwrap_err().contains("lying"));
        assert!(m.check_node(&m.id, "ws://x").is_ok());
    }

    #[test]
    fn address_refuses_another_prefix() {
        let x = [7u8; 32];
        let t = Network::testnet();
        let m = Network::mainnet();
        let on_test = t.p2pk(&x).to_string();
        let on_main = m.p2pk(&x).to_string();
        assert!(on_test.starts_with("kaspatest:") && on_main.starts_with("kaspa:"));
        assert!(t.try_to_spk(&on_test).is_ok() && m.try_to_spk(&on_main).is_ok());
        let e = m.try_to_spk(&on_test).unwrap_err();
        assert!(e.contains("kaspatest address") && e.contains("start with kaspa:"), "{e}");
        assert!(t.try_to_spk(&on_main).unwrap_err().contains("start with kaspatest:"));
        assert!(t.try_to_spk("kaspatest:q").unwrap_err().contains("not a Kaspa address"));
        // the same script renders with each folder's own prefix
        let spk = t.to_spk(&on_test);
        assert_eq!(m.address_of(&spk).unwrap().to_string(), on_main);
        assert_eq!(t.address_of(&spk).unwrap().to_string(), on_test);
    }

    #[test]
    fn recorded_network_mismatch_is_refused() {
        let t = Network::testnet();
        let m = Network::mainnet();
        assert!(t.check_recorded("testnet", "keys.json").is_ok());
        assert!(t.check_recorded("", "keys.json").is_ok(), "older testnet files have no record");
        assert!(m.check_recorded("mainnet", "state.json").is_ok());
        let e = m.check_recorded("", "keys.json").unwrap_err();
        assert!(e.contains("does not say which network"), "{e}");
        let e = t.check_recorded("mainnet", "state.json").unwrap_err();
        assert!(e.contains("made for the mainnet network") && e.contains("--network mainnet"), "{e}");
        assert!(m.check_recorded("testnet", "keys.json").unwrap_err().contains("--network testnet"));
    }
}
