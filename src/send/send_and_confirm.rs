use std::time::Duration;
use colored::*;
use eore_api::error::OreError;
use indicatif::ProgressBar;
use log::{debug, error, info, warn};
use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    rpc_config::RpcSendTransactionConfig,
};
use solana_program::{
    instruction::Instruction,
    message::Message,
    native_token::{lamports_to_sol, sol_to_lamports},
};
use solana_rpc_client::spinner;
use solana_sdk::{
    commitment_config::CommitmentLevel,
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};

use crate::utils::{get_latest_blockhash_with_retries, ComputeBudget};
use crate::Miner;

const MIN_ETH_BALANCE: f64 = 0.0005;
const RPC_RETRIES: usize = 0;
const CONFIRM_RETRIES: usize = 5;
const CONFIRM_DELAY: u64 = 200;
const MAX_ATTEMPTS: usize = 30;

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        compute_budget: ComputeBudget,
        skip_confirm: bool,
    ) -> ClientResult<Signature> {
        debug!("Starting send_and_confirm with {} instructions", ixs.len());

        let progress_bar = spinner::new_progress_bar();
        let signer = self.signer();
        let client = self.rpc_client.clone();
        let fee_payer = self.fee_payer();

        debug!("Using signer: {}", signer.pubkey());
        debug!("Using fee payer: {}", fee_payer.pubkey());
        debug!("RPC client URL: {}", client.url());

        // Check balance
        self.check_balance().await?;

        // Set compute budget
        let mut final_ixs = vec![];
        match compute_budget {
            ComputeBudget::Dynamic => {
                debug!("Using dynamic compute budget");
                // Create a message for simulation
                let (blockhash, _) = get_latest_blockhash_with_retries(&client).await?;
                let message = Message::new_with_blockhash(ixs, Some(&fee_payer.pubkey()), &blockhash);
                let sim_tx = Transaction::new_unsigned(message);
                let sim_result = client.simulate_transaction(&sim_tx).await;
                let units = match sim_result {
                    Ok(sim) => sim.value.units_consumed.unwrap_or(100_000) as u32,
                    Err(err) => {
                        warn!("Simulation failed: {}. Using default 100,000 units", err);
                        100_000
                    }
                };
                debug!("Simulated compute units: {}", units);
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(units));
            }
            ComputeBudget::Fixed(cus) => {
                debug!("Using fixed compute budget: {} CUs", cus);
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(cus));
            }
        }

        // Set compute unit price
        let priority_fee = self.priority_fee.unwrap_or(5000);
        debug!("Setting compute unit price: {} microlamports", priority_fee);
        final_ixs.push(ComputeBudgetInstruction::set_compute_unit_price(priority_fee));

        // Add user instructions
        debug!("Adding {} user instructions", ixs.len());
        for (i, ix) in ixs.iter().enumerate() {
            info!("Original Instruction #{}: Program ID = {}", i, ix.program_id);
            debug!(
                "  - Accounts: {:?}",
                ix.accounts.iter().map(|a| a.pubkey.to_string()).collect::<Vec<_>>()
            );
        }
        final_ixs.extend_from_slice(ixs);

        // Log final instructions
        for (i, ix) in final_ixs.iter().enumerate() {
            info!("Final Instruction #{}: Program ID = {}", i, ix.program_id);
            debug!(
                "  - Accounts: {:?}",
                ix.accounts.iter().map(|a| a.pubkey.to_string()).collect::<Vec<_>>()
            );
        }

        // Build transaction
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Processed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(RPC_RETRIES),
            min_context_slot: None,
        };
        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));

        // Submit transaction
        let mut attempts = 0;
        loop {
            if attempts >= MAX_ATTEMPTS {
                error!("Max attempts ({}) reached", MAX_ATTEMPTS);
                log_error(&progress_bar, "Max attempts reached", true);
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("Max attempts reached".to_string()),
                });
            }

            debug!("Transaction attempt #{}", attempts);
            progress_bar.set_message(format!("Submitting transaction... (attempt {})", attempts));

            // Sign with new blockhash every 10 attempts
            if attempts % 10 == 0 {
                debug!("Refreshing blockhash (attempt {})", attempts);
                let start = std::time::Instant::now();
                let (hash, slot) = get_latest_blockhash_with_retries(&client).await?;
                debug!("Got blockhash {} at slot {} in {:?}", hash, slot, start.elapsed());

                // Update priority fee if dynamic
                if self.dynamic_fee {
                    debug!("Computing dynamic priority fee");
                    let start = std::time::Instant::now();
                    let fee = match self.get_dynamic_priority_fee().await {
                        Ok(fee) => {
                            debug!("Dynamic priority fee: {} microlamports in {:?}", fee, start.elapsed());
                            progress_bar.println(format!("  Priority fee: {} microlamports", fee));
                            fee
                        }
                        Err(err) => {
                            let fee = self.priority_fee.unwrap_or(5000);
                            warn!("Failed to get dynamic fee: {}. Using: {} microlamports", err, fee);
                            log_warning(&progress_bar, &format!("Dynamic fee failed: {}. Using: {}", err, fee));
                            fee
                        }
                    };
                    final_ixs.remove(1);
                    final_ixs.insert(1, ComputeBudgetInstruction::set_compute_unit_price(fee));
                    tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));
                }

                if signer.pubkey() == fee_payer.pubkey() {
                    debug!("Signing with single signer");
                    tx.sign(&[&signer], hash);
                } else {
                    debug!("Signing with signer and fee payer");
                    tx.sign(&[&signer, &fee_payer], hash);
                }
            }

            // Send transaction
            attempts += 1;
            debug!("Sending transaction");
            let start = std::time::Instant::now();
            match client.send_transaction_with_config(&tx, send_cfg).await {
                Ok(sig) => {
                    debug!("Transaction sent: {} in {:?}", sig, start.elapsed());
                    if skip_confirm {
                        progress_bar.finish_with_message(format!("Sent: {}", sig));
                        return Ok(sig);
                    }

                    // Confirm transaction
                    'confirm: for confirm_attempt in 0..CONFIRM_RETRIES {
                        debug!("Confirmation attempt #{} for {}", confirm_attempt, sig);
                        tokio::time::sleep(Duration::from_millis(CONFIRM_DELAY)).await;
                        let start = std::time::Instant::now();
                        match client.get_signature_statuses(&[sig]).await {
                            Ok(signature_statuses) => {
                                debug!("Signature statuses in {:?}", start.elapsed());
                                for status in signature_statuses.value {
                                    if let Some(status) = status {
                                        if let Some(err) = status.err {
                                            debug!("Transaction error: {:?}", err);
                                            match err {
                                                solana_sdk::transaction::TransactionError::InstructionError(_, err) => {
                                                    match err {
                                                        solana_program::instruction::InstructionError::Custom(err_code) => {
                                                            match err_code {
                                                                e if e == OreError::NeedsReset as u32 => {
                                                                    debug!("Needs reset, retrying");
                                                                    attempts = 0;
                                                                    log_error(&progress_bar, "Needs reset. Retrying...", false);
                                                                    break 'confirm;
                                                                }
                                                                _ => {
                                                                    error!("Custom instruction error: {}", err);
                                                                    log_error(&progress_bar, &err.to_string(), true);
                                                                    return Err(ClientError {
                                                                        request: None,
                                                                        kind: ClientErrorKind::Custom(err.to_string()),
                                                                    });
                                                                }
                                                            }
                                                        }
                                                        _ => {
                                                            error!("Non-custom instruction error: {}", err);
                                                            log_error(&progress_bar, &err.to_string(), true);
                                                            return Err(ClientError {
                                                                request: None,
                                                                kind: ClientErrorKind::Custom(err.to_string()),
                                                            });
                                                        }
                                                    }
                                                }
                                                _ => {
                                                    error!("Non-instruction error: {}", err);
                                                    log_error(&progress_bar, &err.to_string(), true);
                                                    return Err(ClientError {
                                                        request: None,
                                                        kind: ClientErrorKind::Custom(err.to_string()),
                                                    });
                                                }
                                            }
                                        } else if let Some(confirmation) = status.confirmation_status {
                                            debug!("Confirmation status: {:?}", confirmation);
                                            match confirmation {
                                                TransactionConfirmationStatus::Processed
                                                | TransactionConfirmationStatus::Confirmed
                                                | TransactionConfirmationStatus::Finalized => {
                                                    debug!("Transaction confirmed");
                                                    progress_bar.finish_with_message(format!(
                                                        "{} {}",
                                                        "OK".bold().green(),
                                                        sig
                                                    ));
                                                    return Ok(sig);
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                            }
                            Err(err) => {
                                warn!("Signature status error: {}", err);
                                log_error(&progress_bar, &err.kind().to_string(), false);
                            }
                        }
                    }
                }
                Err(err) => {
                    error!("Send transaction error: {} in {:?}", err, start.elapsed());
                    log_error(&progress_bar, &err.kind().to_string(), false);
                }
            }
        }
    }

    pub async fn check_balance(&self) -> ClientResult<()> {
        debug!("Checking balance for signer: {}", self.signer().pubkey());
        let start = std::time::Instant::now();
        let balance = self
            .rpc_client
            .get_balance(&self.signer().pubkey())
            .await
            .map_err(|err| {
                error!("Failed to get balance: {}", err);
                ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom(format!("Failed to get balance: {}", err)),
                }
            })?;
        debug!("Balance: {} ETH in {:?}", lamports_to_sol(balance), start.elapsed());
        if balance < sol_to_lamports(MIN_ETH_BALANCE) {
            error!(
                "Insufficient balance: {} ETH < {} ETH",
                lamports_to_sol(balance),
                MIN_ETH_BALANCE
            );
            log_error(
                &spinner::new_progress_bar(),
                &format!(
                    "Insufficient balance: {} ETH < {} ETH",
                    lamports_to_sol(balance),
                    MIN_ETH_BALANCE
                ),
                true,
            );
            return Err(ClientError {
                request: None,
                kind: ClientErrorKind::Custom("Insufficient balance".to_string()),
            });
        }
        Ok(())
    }
}

fn log_error(progress_bar: &ProgressBar, err: &str, finish: bool) {
    if finish {
        progress_bar.finish_with_message(format!("{} {}", "ERROR".bold().red(), err));
    } else {
        progress_bar.println(format!("  {} {}", "ERROR".bold().red(), err));
    }
}

fn log_warning(progress_bar: &ProgressBar, msg: &str) {
    progress_bar.println(format!("  {} {}", "WARNING".bold().yellow(), msg));
}
