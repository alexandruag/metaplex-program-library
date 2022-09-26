pub mod utils;

use solana_program::{program_option::COption, program_pack::Pack};
use solana_program_test::tokio;
use solana_sdk::signature::{Keypair, Signer};
use spl_associated_token_account::get_associated_token_address;
use spl_token::{self, state::Mint};

use utils::{
    context::{BubblegumTestContext, DEFAULT_LAMPORTS_FUND_AMOUNT},
    tree::{decompress_mint_auth_pda, Tree},
    LeafArgs, Result,
};

// Test for multiple combinations?
const MAX_DEPTH: usize = 14;
const MAX_BUF_SIZE: usize = 64;

// Minting too many leaves takes quite a long time (in these tests at least).
const DEFAULT_NUM_MINTS: u64 = 10;

// TODO: test signer conditions on mint_authority and other stuff that's manually checked
// and not by anchor (what else is there?)

// TODO: will add some exta checks to the tests below (i.e. read accounts and
// assert on values therein).

// Creates a `BubblegumTestContext`, a `Tree` with default arguments, and also mints an NFT
// with the default `LeafArgs`.
async fn context_tree_and_leaves() -> Result<(
    BubblegumTestContext,
    Tree<MAX_DEPTH, MAX_BUF_SIZE>,
    Vec<LeafArgs>,
)> {
    let context = BubblegumTestContext::new().await?;

    let (tree, leaves) = context
        .default_create_and_mint::<MAX_DEPTH, MAX_BUF_SIZE>(DEFAULT_NUM_MINTS)
        .await?;

    Ok((context, tree, leaves))
}

#[tokio::test]
async fn test_create_tree_and_mint_passes() {
    let (context, tree, _) = context_tree_and_leaves().await.unwrap();

    let payer = context.payer();

    let cfg = tree.read_tree_config().await.unwrap();
    assert_eq!(cfg.tree_creator, payer.pubkey());
    assert_eq!(cfg.tree_delegate, payer.pubkey());
    assert_eq!(cfg.total_mint_capacity, 1 << MAX_DEPTH);
    assert_eq!(cfg.num_minted, DEFAULT_NUM_MINTS);
}

#[tokio::test]
async fn test_creator_verify_and_unverify_passes() {
    let (context, tree, mut leaves) = context_tree_and_leaves().await.unwrap();

    for leaf in leaves.iter_mut() {
        tree.verify_creator(leaf, &context.default_creators[0])
            .await
            .unwrap();
    }

    for leaf in leaves.iter_mut() {
        tree.unverify_creator(leaf, &context.default_creators[0])
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn test_delegate_passes() {
    let (_, tree, mut leaves) = context_tree_and_leaves().await.unwrap();
    let new_delegate = Keypair::new();

    for leaf in leaves.iter_mut() {
        tree.delegate(leaf, &new_delegate).await.unwrap();
    }
}

#[tokio::test]
async fn test_transfer_passes() {
    let (_, tree, mut leaves) = context_tree_and_leaves().await.unwrap();
    let new_owner = Keypair::new();

    for leaf in leaves.iter_mut() {
        tree.transfer(leaf, &new_owner).await.unwrap();
    }
}

#[tokio::test]
async fn test_delegated_transfer_passes() {
    let (mut context, tree, mut leaves) = context_tree_and_leaves().await.unwrap();
    let delegate = Keypair::new();
    let new_owner = Keypair::new();

    context
        .fund_account(delegate.pubkey(), DEFAULT_LAMPORTS_FUND_AMOUNT)
        .await
        .unwrap();

    for leaf in leaves.iter_mut() {
        // We need to explicitly set a new delegate, since by default the owner has both
        // roles right after minting.
        tree.delegate(leaf, &delegate).await.unwrap();

        let mut tx = tree.transfer_tx(leaf, &new_owner).await.unwrap();

        // Set the delegate as payer and signer (by default, it's the owner).
        tx.set_payer(delegate.pubkey()).set_signers(&[&delegate]);

        tx.execute().await.unwrap();
    }
}

#[tokio::test]
async fn test_burn_passes() {
    let (_, tree, leaves) = context_tree_and_leaves().await.unwrap();

    for leaf in leaves.iter() {
        tree.burn(&leaf).await.unwrap();
    }
}

#[tokio::test]
async fn test_set_tree_delegate_passes() {
    let (context, tree, _) = context_tree_and_leaves().await.unwrap();
    let new_tree_delegate = Keypair::new();

    let initial_cfg = tree.read_tree_config().await.unwrap();
    tree.set_tree_delegate(&new_tree_delegate).await.unwrap();
    let mut cfg = tree.read_tree_config().await.unwrap();

    // Configs are not the same.
    assert_ne!(cfg, initial_cfg);
    assert_eq!(cfg.tree_delegate, new_tree_delegate.pubkey());
    // Configs are the same if we change back the delegate (nothing else changed).
    cfg.tree_delegate = context.payer().pubkey();
    assert_eq!(cfg, initial_cfg);
}

#[tokio::test]
async fn test_reedem_and_cancel_passes() {
    let (_, tree, leaves) = context_tree_and_leaves().await.unwrap();

    for leaf in leaves.iter() {
        tree.redeem(leaf).await.unwrap();

        let v = tree.read_voucher(leaf.nonce).await.unwrap();
        assert_eq!(v, tree.expected_voucher(leaf));
    }

    for leaf in leaves.iter() {
        tree.cancel_redeem(leaf).await.unwrap();
    }
}

#[tokio::test]
async fn test_decompress_passes() {
    let (_, tree, leaves) = context_tree_and_leaves().await.unwrap();

    for leaf in leaves.iter() {
        tree.redeem(leaf).await.unwrap();
        let voucher = tree.read_voucher(leaf.nonce).await.unwrap();

        tree.decompress_v1(&voucher, leaf).await.unwrap();

        let mint_key = voucher.decompress_mint_pda();
        let mint_account = tree.read_account(mint_key).await.unwrap();
        let mint = Mint::unpack(mint_account.data.as_slice()).unwrap();

        // TODO: figure out where the final `mint_authority` value comes from for `mint`.
        assert_eq!(mint.supply, 1);
        assert_eq!(mint.decimals, 0);

        assert!(mint.is_initialized);
        assert!(mint.freeze_authority.is_none());

        let token_account_key = get_associated_token_address(&leaf.owner.pubkey(), &mint_key);
        let token_account = tree.read_account(token_account_key).await.unwrap();
        let t = spl_token::state::Account::unpack(token_account.data.as_slice()).unwrap();

        assert_eq!(t.mint, mint_key);
        assert_eq!(t.owner, leaf.owner.pubkey());
        assert_eq!(t.amount, 1);
        assert_eq!(t.state, spl_token::state::AccountState::Initialized);
        assert_eq!(t.delegated_amount, 0);

        assert!(t.delegate.is_none());
        assert!(t.is_native.is_none());
        assert!(t.close_authority.is_none());

        // TODO: asserts for TM accounts
    }
}
