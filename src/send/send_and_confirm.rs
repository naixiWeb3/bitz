use std::{str::FromStr, time::Duration};

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
    native_token::{lamports_to_sol, sol_to_lamports},
    pubkey::Pubkey,
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
const _SIMULATION_RETRIES: usize = 4;
const GATEWAY_RETRIES: usize = 150;
const CONFIRM_RETRIES: usize = 8;

const CONFIRM_DELAY: u64 = 500;
const GATEWAY_DELAY: u64 = 0;

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

        // Return error, if balance is zero
        self.check_balance().await;

        // Set compute budget
        let mut final_ixs = vec![];
        match compute_budget {
            ComputeBudget::Dynamic => {
                debug!("Using dynamic compute budget");
                todo!("simulate tx")
            }
            ComputeBudget::Fixed(cus) => {
                debug!("Using fixed compute budget: {} CUs", cus);
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(cus))
            }
        }

        // Set compute unit price
        let priority_fee = self.priority_fee.unwrap_or(0);
        debug!("Setting compute unit price: {} microlamports", priority_fee);
        final_ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
            priority_fee,
        ));

        // Add in user instructions
        debug!("Adding {} user instructions", ixs.len());

        // Log program addresses for original instructions
        for (i, ix) in ixs.iter().enumerate() {
            info!(
                "Original Instruction #{}: Program ID = {}",
                i, ix.program_id
            );
            debug!(
                "  - Accounts: {:?}",
                ix.accounts
                    .iter()
                    .map(|a| a.pubkey.to_string())
                    .collect::<Vec<_>>()
            );
        }

        final_ixs.extend_from_slice(ixs);

        // Log all final instructions including compute budget instructions
        for (i, ix) in final_ixs.iter().enumerate() {
            info!("Final Instruction #{}: Program ID = {}", i, ix.program_id);
            debug!(
                "  - Accounts: {:?}",
                ix.accounts
                    .iter()
                    .map(|a| a.pubkey.to_string())
                    .collect::<Vec<_>>()
            );
        }

        // Build tx
        debug!("Building transaction with config: skip_preflight=true, commitment=Confirmed");
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(RPC_RETRIES),
            min_context_slot: None,
        };
        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));

        // Submit tx
        let mut attempts = 0;
        loop {
            debug!("Transaction attempt #{}", attempts);
            progress_bar.set_message(format!("Submitting transaction... (attempt {})", attempts,));

            // Sign tx with a new blockhash (after approximately ~45 sec)
            if attempts % 10 == 0 {
                debug!(
                    "Refreshing blockhash and recomputing fees (attempt {})",
                    attempts
                );

                // Reset the compute unit price
                if self.dynamic_fee {
                    debug!("Computing dynamic priority fee");
                    let fee = match self.get_dynamic_priority_fee().await {
                        Ok(fee) => {
                            debug!("Dynamic priority fee computed: {} microlamports", fee);
                            progress_bar.println(format!("  Priority fee: {} microlamports", fee));
                            fee
                        }
                        Err(err) => {
                            let fee = self.priority_fee.unwrap_or(0);
                            warn!("Failed to get dynamic fee: {}. Falling back to static value: {} microlamports", err, fee);
                            log_warning(
                                &progress_bar,
                                &format!(
                                    "{} Falling back to static value: {} microlamports",
                                    err, fee
                                ),
                            );
                            fee
                        }
                    };

                    final_ixs.remove(1);
                    final_ixs.insert(1, ComputeBudgetInstruction::set_compute_unit_price(fee));
                    tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));
                }

                // Resign the tx
                debug!("Getting latest blockhash");
                let (hash, slot) = get_latest_blockhash_with_retries(&client).await?;
                debug!("Got blockhash {} at slot {}", hash, slot);

                if signer.pubkey() == fee_payer.pubkey() {
                    debug!("Signing transaction with single signer");
                    tx.sign(&[&signer], hash);
                } else {
                    debug!("Signing transaction with both signer and fee payer");
                    tx.sign(&[&signer, &fee_payer], hash);
                }
            }

            // Send transaction
            attempts += 1;
            debug!("Sending transaction to RPC");
            match client.send_and_confirm_transaction(&tx).await {
                Ok(sig) => {
                    debug!("Transaction sent successfully: {}", sig);

                    // Skip confirmation
                    if skip_confirm {
                        debug!("Skipping confirmation as requested");
                        progress_bar.finish_with_message(format!("Sent: {}", sig));
                        return Ok(sig);
                    }

                    // Confirm transaction
                    'confirm: for confirm_attempt in 0..CONFIRM_RETRIES {
                        debug!(
                            "Confirmation attempt #{} for signature {}",
                            confirm_attempt, sig
                        );
                        tokio::time::sleep(Duration::from_millis(CONFIRM_DELAY)).await;
                        match client.get_signature_statuses(&[sig]).await {
                            Ok(signature_statuses) => {
                                debug!("Got signature statuses: {:?}", signature_statuses);
                                for status in signature_statuses.value {
                                    if let Some(status) = status {
                                        if let Some(err) = status.err {
                                            debug!("Transaction error: {:?}", err);
                                            match err {
                                                // Instruction error
                                                solana_sdk::transaction::TransactionError::InstructionError(_, err) => {
                                                    match err {
                                                        // Custom instruction error, parse into OreError
                                                        solana_program::instruction::InstructionError::Custom(err_code) => {
                                                            match err_code {
                                                                e if e == OreError::NeedsReset as u32 => {
                                                                    debug!("Needs reset error, retrying transaction");
                                                                    attempts = 0;
                                                                    log_error(&progress_bar, "Needs reset. Retrying...", false);
                                                                    break 'confirm;
                                                                },
                                                                _ => {
                                                                    error!("Custom instruction error: {}", err);
                                                                    log_error(&progress_bar, &err.to_string(), true);
                                                                    return Err(ClientError {
                                                                        request: None,
                                                                        kind: ClientErrorKind::Custom(err.to_string()),
                                                                    });
                                                                }
                                                            }
                                                        },

                                                        // Non custom instruction error, return
                                                        _ => {
                                                            error!("Non-custom instruction error: {}", err);
                                                            log_error(&progress_bar, &err.to_string(), true);
                                                            return Err(ClientError {
                                                                request: None,
                                                                kind: ClientErrorKind::Custom(err.to_string()),
                                                            });
                                                        }
                                                    }
                                                },

                                                // Non instruction error, return
                                                _ => {
                                                    error!("Non-instruction error: {}", err);
                                                    log_error(&progress_bar, &err.to_string(), true);
                                                    return Err(ClientError {
                                                        request: None,
                                                        kind: ClientErrorKind::Custom(err.to_string()),
                                                    });
                                                }
                                            }
                                        } else if let Some(confirmation) =
                                            status.confirmation_status
                                        {
                                            debug!(
                                                "Transaction confirmation status: {:?}",
                                                confirmation
                                            );
                                            match confirmation {
                                                TransactionConfirmationStatus::Processed => {
                                                    debug!(
                                                        "Transaction processed but not confirmed"
                                                    );
                                                }
                                                TransactionConfirmationStatus::Confirmed
                                                | TransactionConfirmationStatus::Finalized => {
                                                    debug!("Transaction confirmed/finalized");
                                                    progress_bar.finish_with_message(format!(
                                                        "{} {}",
                                                        "OK".bold().green(),
                                                        sig
                                                    ));
                                                    return Ok(sig);
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Handle confirmation errors
                            Err(err) => {
                                warn!("Error getting signature status: {}", err);
                                log_error(&progress_bar, &err.kind().to_string(), false);
                            }
                        }
                    }
                }

                // Handle submit errors
                Err(err) => {
                    error!("Error submitting transaction: {}", err);
                    log_error(&progress_bar, &err.kind().to_string(), false);
                }
            }
        }
    }

    pub async fn check_balance(&self) {
        debug!("Checking balance for signer: {}", self.signer().pubkey());
        let balance = self
            .rpc_client
            .get_balance(&self.signer().pubkey())
            .await
            .unwrap_or(0);
        debug!("Current balance: {} ETH", lamports_to_sol(balance));
        if balance < sol_to_lamports(MIN_ETH_BALANCE) {
            error!(
                "Insufficient balance: {} ETH < {} ETH",
                lamports_to_sol(balance),
                MIN_ETH_BALANCE
            );
            panic!(
                "Insufficient balance: {} ETH < {} ETH",
                lamports_to_sol(balance),
                MIN_ETH_BALANCE
            );
        }
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
