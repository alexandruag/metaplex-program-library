use std::convert::TryFrom;
use std::str::FromStr;

use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;
use solana_program::rent::Rent;
use solana_program::system_instruction;
use solana_program::system_program;
use solana_program_test::tokio;
use solana_program_test::ProgramTest;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

const MAX_DEPTH: u32 = 20;
const MAX_SIZE: u32 = 64;

fn program_test() -> ProgramTest {
    ProgramTest::new("mpl_bubblegum", mpl_bubblegum::id(), None)
}

fn pid_candy_wrapper() -> Pubkey {
    Pubkey::from_str("WRAPYChf58WFCnyjXKJHtrPgzKXgHp6MD9aVDqJBbGh").unwrap()
}

fn pid_gummyroll() -> Pubkey {
    Pubkey::from_str("GRoLLzvxpxxu2PGNJMMeZPyMxjAUH9pKqxGXV9DGiceU").unwrap()
}

pub fn ix_alloc_tree(
    payer: Pubkey,
    merkle_roll: Pubkey,
    max_depth: u32,
    max_buf_size: u32,
    rent: &Rent,
) -> Instruction {
    let merkle_roll_account_size = |canopy_depth: u32| {
        // TODO: check overflows and use u64 everywhere?
        let header_size = 8 + 32;
        let changelog_size = (max_depth * 32 + 32 + 4 + 4) * max_buf_size;
        let rightmost_path_size = max_depth * 32 + 32 + 4 + 4;
        let merkle_roll_size = 8 + 8 + 16 + changelog_size + rightmost_path_size;

        // This is 0 when `canopy_depth == 0`.
        let canopy_size = ((1 << canopy_depth + 1) - 2) * 32;

        u64::from(merkle_roll_size + header_size + canopy_size)
    };

    let account_size = merkle_roll_account_size(0);

    // u64 -> usize conversion should never fail on the platforms we're running on.
    let lamports = rent.minimum_balance(usize::try_from(account_size).unwrap());

    system_instruction::create_account(
        &payer,
        &merkle_roll,
        lamports,
        account_size,
        &pid_gummyroll(),
    )
}

pub fn ix_init_tree(
    payer: Pubkey,
    tree_creator: Pubkey,
    slab: Pubkey,
    max_depth: u32,
    max_buf_size: u32,
) -> Instruction {
    let (auth, _) = Pubkey::find_program_address(&[slab.as_ref()], &mpl_bubblegum::id());

    // Disc for `create_tree`.
    let instruction_discriminator = [165u8, 83, 136, 142, 89, 202, 47, 220];

    let mut data = Vec::new();
    data.extend_from_slice(&instruction_discriminator);
    data.extend_from_slice(&max_depth.to_le_bytes());
    data.extend_from_slice(&max_buf_size.to_le_bytes());

    let accounts = vec![
        // Is this a signer?
        AccountMeta::new(auth, false),
        AccountMeta::new(payer, true),
        // Is this a signer?
        AccountMeta::new_readonly(tree_creator, true),
        AccountMeta::new_readonly(pid_candy_wrapper(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(pid_gummyroll(), false),
        AccountMeta::new(slab, false),
    ];

    Instruction {
        program_id: mpl_bubblegum::id(),
        accounts,
        data,
    }
}

#[tokio::test]
async fn test_init() {
    let mut context = program_test().start_with_context().await;
    let cl = &mut context.banks_client;
    let rent = cl.get_rent().await.unwrap();

    let merkle_roll = Keypair::new();

    let payer = context.payer;

    let tx = Transaction::new_signed_with_payer(
        &[ix_alloc_tree(
            payer.pubkey(),
            merkle_roll.pubkey(),
            MAX_DEPTH,
            MAX_SIZE,
            &rent,
        )],
        Some(&payer.pubkey()),
        // I think merkle_roll has to sign here just because it's' the `to_pubkey` from the
        // `create_account` instruction.
        &[&payer, &merkle_roll],
        context.last_blockhash,
    );

    cl.process_transaction(tx).await.unwrap();

    let tx2 = Transaction::new_signed_with_payer(
        &[ix_init_tree(
            payer.pubkey(),
            payer.pubkey(),
            merkle_roll.pubkey(),
            MAX_DEPTH,
            MAX_SIZE,
        )],
        Some(&payer.pubkey()),
        &[&payer],
        context.last_blockhash,
    );

    cl.process_transaction(tx2).await.unwrap();
}
