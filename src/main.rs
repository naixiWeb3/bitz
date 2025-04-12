mod args;
mod command;
mod error;
mod send;
mod utils;

use colored::*;
use crossterm::style::Stylize;
use futures::StreamExt;
use std::{sync::Arc, sync::RwLock};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

use args::*;
use clap::{command, Parser, Subcommand};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{read_keypair_file, Keypair, Signer},
};
use utils::{PoolCollectingData, SoloCollectingData, Tip};

// TODO: Unify balance and proof into "account"
// TODO: Move balance subcommands to "pool"
// TODO: Make checkpoint an admin command
// TODO: Remove boost command

#[derive(Clone)]
struct Miner {
    pub keypair_filepath: Option<String>,
    pub priority_fee: Option<u64>,
    pub dynamic_fee_url: Option<String>,
    pub dynamic_fee: bool,
    pub rpc_client: Arc<RpcClient>,
    pub fee_payer_filepath: Option<String>,
    pub solo_collecting_data: Arc<std::sync::RwLock<Vec<SoloCollectingData>>>,
    pub pool_collecting_data: Arc<std::sync::RwLock<Vec<PoolCollectingData>>>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Fetch your account details")]
    Account(AccountArgs),

    #[command(about = "Benchmark your machine's hashpower")]
    Benchmark(BenchmarkArgs),

    #[command(about = "Claim your collecting yield")]
    Claim(ClaimArgs),

    #[cfg(feature = "admin")]
    #[command(about = "Initialize the program")]
    Initialize(InitializeArgs),

    #[command(about = "Start collecting on your local machine")]
    Collect(CollectArgs),

    #[command(about = "Connect to a collecting pool")]
    Pool(PoolArgs),

    #[command(about = "Fetch onchain global program variables")]
    Program(ProgramArgs),

    #[command(about = "Manage your stake positions")]
    Stake(StakeArgs),

    #[command(about = "Fetch details about a BITZ transaction")]
    Transaction(TransactionArgs),

    #[command(about = "Send BITZ to another user")]
    Transfer(TransferArgs),
}

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(
        long,
        value_name = "NETWORK_URL",
        help = "Network address of your RPC provider",
        default_value = "https://mainnetbeta-rpc.eclipse.xyz/",
        global = true
    )]
    rpc: Option<String>,

    #[clap(
        global = true,
        short = 'C',
        long = "config",
        id = "PATH",
        help = "Filepath to config file."
    )]
    config_file: Option<String>,

    #[arg(
        long,
        value_name = "KEYPAIR_FILEPATH",
        help = "Filepath to signer keypair. Base58 or Raw JSON.",
        default_value = "key.txt",
        global = true
    )]
    keypair: Option<String>,

    #[arg(
        long,
        value_name = "FEE_PAYER_FILEPATH",
        help = "Filepath to transaction fee payer keypair.",
        global = true
    )]
    fee_payer: Option<String>,

    #[arg(
        long,
        value_name = "MICROLAMPORTS",
        help = "Price to pay for compute units. If dynamic fees are enabled, this value will be used as the cap.",
        default_value = "1000",
        global = true
    )]
    priority_fee: Option<u64>,

    #[arg(
        long,
        value_name = "DYNAMIC_FEE_URL",
        help = "RPC URL to use for dynamic fee estimation.",
        global = true
    )]
    dynamic_fee_url: Option<String>,

    #[arg(long, help = "Enable dynamic priority fees", global = true)]
    dynamic_fee: bool,

    #[command(subcommand)]
    command: Commands,
}

#[tokio::main]
async fn main() {
    // Initialize logger
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let args = Args::parse();

    // Load the config file from custom path, the default path, or use default config values
    let cli_config = if let Some(config_file) = &args.config_file {
        if std::path::Path::new(config_file).exists() {
            solana_cli_config::Config::load(config_file).unwrap_or_else(|_| {
                eprintln!("error: Could not load config file `{}`", config_file);
                std::process::exit(1);
            })
        } else {
            // 如果指定的配置文件不存在，尝试默认配置文件
            if let Some(default_config_file) = &*solana_cli_config::CONFIG_FILE {
                solana_cli_config::Config::load(default_config_file).unwrap_or_default()
            } else {
                solana_cli_config::Config::default()
            }
        }
    } else if let Some(config_file) = &*solana_cli_config::CONFIG_FILE {
        solana_cli_config::Config::load(config_file).unwrap_or_default()
    } else {
        solana_cli_config::Config::default()
    };

    // Initialize miner.
    let cluster = args.rpc.unwrap_or(cli_config.json_rpc_url);
    let default_keypair = args.keypair.unwrap_or(cli_config.keypair_path.clone());
    let fee_payer_filepath = args.fee_payer.unwrap_or(default_keypair.clone());
    let rpc_client = RpcClient::new_with_commitment(cluster, CommitmentConfig::confirmed());

    let solo_collecting_data = Arc::new(RwLock::new(Vec::new()));
    let pool_collecting_data = Arc::new(RwLock::new(Vec::new()));

    let miner = Arc::new(Miner::new(
        Arc::new(rpc_client),
        args.priority_fee,
        Some(default_keypair),
        args.dynamic_fee_url,
        args.dynamic_fee,
        Some(fee_payer_filepath),
        solo_collecting_data,
        pool_collecting_data,
    ));

    let signer = miner.signer();
    println!("Address: {}", signer.pubkey().to_string().green());

    // Execute user command.
    match args.command {
        Commands::Account(args) => {
            miner.account(args).await;
        }
        Commands::Benchmark(args) => {
            miner.benchmark(args).await;
        }
        Commands::Claim(args) => {
            if let Err(err) = miner.claim(args).await {
                println!("{:?}", err);
            }
        }
        Commands::Pool(args) => {
            miner.pool(args).await;
        }
        Commands::Program(_) => {
            miner.program().await;
        }
        Commands::Collect(args) => {
            if let Err(err) = miner.collect(args).await {
                println!("{:?}", err);
            }
        }
        Commands::Stake(args) => {
            miner.stake(args).await;
        }
        Commands::Transfer(args) => {
            miner.transfer(args).await;
        }
        Commands::Transaction(args) => {
            miner.transaction(args).await.unwrap();
        }
        #[cfg(feature = "admin")]
        Commands::Initialize(_) => {
            miner.initialize().await;
        }
    }
}

impl Miner {
    pub fn new(
        rpc_client: Arc<RpcClient>,
        priority_fee: Option<u64>,
        keypair_filepath: Option<String>,
        dynamic_fee_url: Option<String>,
        dynamic_fee: bool,
        fee_payer_filepath: Option<String>,
        solo_collecting_data: Arc<std::sync::RwLock<Vec<SoloCollectingData>>>,
        pool_collecting_data: Arc<std::sync::RwLock<Vec<PoolCollectingData>>>,
    ) -> Self {
        Self {
            rpc_client,
            keypair_filepath,
            priority_fee,
            dynamic_fee_url,
            dynamic_fee,
            fee_payer_filepath,
            solo_collecting_data,
            pool_collecting_data,
        }
    }

    pub fn signer(&self) -> Keypair {
        match self.keypair_filepath.clone() {
            Some(filepath) => Miner::read_keypair_from_file(filepath.clone()),
            None => panic!("No keypair provided"),
        }
    }

    pub fn read_keypair_from_file(filepath: String) -> Keypair {
        // 首先判断文件是否存在
        if !std::path::Path::new(&filepath).exists() {
            panic!("File not found at {}", filepath);
        }

        // 先尝试 read_keypair_file
        match read_keypair_file(&filepath) {
            Ok(keypair) => keypair,
            Err(_) => {
                // 如果读取文件失败，尝试读取文件内容作为 base58 字符串
                match std::fs::read_to_string(&filepath) {
                    Ok(content) => {
                        // 移除可能的空白字符
                        let content = content.trim();
                        Keypair::from_base58_string(content)
                    }
                    Err(_) => panic!("Could not read file content: {}", filepath),
                }
            }
        }
    }

    pub fn fee_payer(&self) -> Keypair {
        match self.fee_payer_filepath.clone() {
            Some(filepath) => Miner::read_keypair_from_file(filepath.clone()),
            None => panic!("No fee payer keypair provided"),
        }
    }
}
