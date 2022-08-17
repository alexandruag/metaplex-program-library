use solana_program_test::tokio;

use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;

use crate::state::metaplex_adapter::{Creator, MetadataArgs, TokenProgramVersion};
use crate::state::TreeConfig;

use super::{clone_keypair, program_test, Tree, TreeClient};

const MAX_DEPTH: u32 = 20;
const MAX_SIZE: u32 = 64;

// TODO: test signer conditions on mint_authority and other stuff that's manually checked
// and not by anchor (what else is there?)
#[tokio::test]
async fn test_mint() {
    let context = program_test().start_with_context().await;

    let merkle_roll = Keypair::new();

    let payer = &context.payer;

    let mut tree = TreeClient::new(
        Tree {
            tree_creator: clone_keypair(payer),
            merkle_roll,
            max_depth: MAX_DEPTH,
            max_buffer_size: MAX_SIZE,
            canopy_depth: 0,
        },
        context.banks_client.clone(),
    );

    tree.alloc(payer).await;
    tree.create(payer).await;

    println!("*** tree config {:?}", tree.read_tree_config().await);
    println!(
        "*** mint_auth_req {:?}",
        tree.read_mint_authority_request(&tree.authority()).await
    );

    tree.set_default_mint_request(payer, 1024 * 1024).await;

    println!("*** tree config {:?}", tree.read_tree_config().await);

    println!(
        "*** mint_auth_req {:?}",
        tree.read_mint_authority_request(&tree.authority()).await
    );

    tree.approve_mint_request(tree.mint_authority_request(&tree.authority()), payer, 1024)
        .await;

    println!(
        "*** mint_auth_req {:?}",
        tree.read_mint_authority_request(&tree.authority()).await
    );

    let message = MetadataArgs {
        name: "test".to_owned(),
        // symbol: "test_symbol".to_owned(),
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

    tree.mint_v1(tree.authority(), payer, payer.pubkey(), message)
        .await;
}
