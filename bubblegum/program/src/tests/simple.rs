use solana_program_test::tokio;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;

use crate::state::metaplex_adapter::{Creator, MetadataArgs, TokenProgramVersion};

use super::{clone_keypair, program_test, Error, Result, Tree, TreeClient};

const MAX_DEPTH: u32 = 20;
const MAX_SIZE: u32 = 64;

// TODO: test signer conditions on mint_authority and other stuff that's manually checked
// and not by anchor (what else is there?)
#[tokio::test]
async fn test_simple() -> Result<()> {
    let mut context = program_test().start_with_context().await;

    let merkle_roll = Keypair::new();
    let new_tree_delegate = Keypair::new();
    let new_owner = Keypair::new();

    let payer = &context.payer;

    // Create a transaction to send some funds to the `new_owner` account, which is used
    // as a payer in one of the operations below. Having the payer be an account with no
    // funds causes the Banks server to hang. Will find a better way to implement this
    // op.
    let tx = Transaction::new_signed_with_payer(
        &[system_instruction::transfer(
            &payer.pubkey(),
            &new_owner.pubkey(),
            1_000_000,
        )],
        Some(&payer.pubkey()),
        &[payer],
        context.last_blockhash,
    );
    context
        .banks_client
        .process_transaction(tx)
        .await
        .map_err(Error::BanksClient)?;

    let mut tree = TreeClient::new(
        Tree {
            tree_creator: clone_keypair(payer),
            tree_delegate: clone_keypair(payer),
            merkle_roll,
            max_depth: MAX_DEPTH,
            max_buffer_size: MAX_SIZE,
            canopy_depth: 0,
        },
        context.banks_client.clone(),
    );

    tree.alloc(payer).await?;

    // tree.create(payer).await?;
    tree.create_tx(payer).execute().await?;

    // println!("*** tree config {:?}", tree.read_tree_config().await);
    // println!(
    //     "*** mint_auth_req {:?}",
    //     tree.read_mint_authority_request(&tree.authority()).await
    // );

    tree.set_default_mint_request(1024 * 1024).await?;

    // println!("*** tree config {:?}", tree.read_tree_config().await);
    //
    // println!(
    //     "*** mint_auth_req {:?}",
    //     tree.read_mint_authority_request(&tree.authority()).await
    // );

    tree.approve_mint_request(tree.mint_authority_request(&tree.authority()), payer, 1024)
        .await?;

    // println!(
    //     "*** mint_auth_req {:?}",
    //     tree.read_mint_authority_request(&tree.authority()).await
    // );

    let message = MetadataArgs {
        name: "test".to_owned(),
        symbol: "tst".to_owned(),
        uri: "www.solana.pos".to_owned(),
        seller_fee_basis_points: 0,
        primary_sale_happened: false,
        is_mutable: false,
        edition_nonce: None,
        token_standard: None,
        token_program_version: TokenProgramVersion::Original,
        collection: None,
        uses: None,
        creators: vec![
            Creator {
                address: Keypair::new().pubkey(),
                verified: false,
                share: 20,
            },
            Creator {
                address: Keypair::new().pubkey(),
                verified: false,
                share: 20,
            },
            Creator {
                address: Keypair::new().pubkey(),
                verified: false,
                share: 20,
            },
            Creator {
                address: Keypair::new().pubkey(),
                verified: false,
                share: 40,
            },
        ],
    };

    tree.mint_v1(tree.authority(), payer, payer.pubkey(), &message)
        .await?;

    let new_nft_delegate = Keypair::new().pubkey();
    tree.delegate(payer, payer.pubkey(), new_nft_delegate, &message, 0)
        .await?;

    tree.transfer(payer, new_nft_delegate, new_owner.pubkey(), &message, 0)
        .await?;

    tree.burn(&new_owner, new_owner.pubkey(), &message, 0)
        .await?;

    tree.set_tree_delegate(&new_tree_delegate).await?;

    Ok(())
}
