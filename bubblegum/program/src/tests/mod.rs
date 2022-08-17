use std::convert::TryFrom;

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use solana_program_test::{BanksClient, ProgramTest};
use solana_program::instruction::Instruction;
use solana_program::pubkey::Pubkey;
use solana_program::rent::Rent;
use solana_program::system_instruction;
use solana_program::system_program;
use solana_sdk::account::Account;
use solana_sdk::hash::Hash;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::signer::signers::Signers;
use solana_sdk::transaction::Transaction;

use crate::state::metaplex_adapter::MetadataArgs;
use crate::state::request::MintRequest;
use crate::state::TreeConfig;

mod dummy;
mod mint;

fn program_test() -> ProgramTest {
    let mut test = ProgramTest::new("mpl_bubblegum", crate::id(), None);
    test.add_program("mpl_candy_wrapper", mpl_candy_wrapper::id(), None);
    test.add_program("gummyroll", gummyroll::id(), None);
    test
}

fn instruction<T, U>(accounts: T, data: U) -> Instruction
where
    T: ToAccountMetas,
    U: InstructionData,
{
    Instruction {
        program_id: crate::id(),
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

pub fn clone_keypair(k: &Keypair) -> Keypair {
    Keypair::from_bytes(k.to_bytes().as_slice()).unwrap()
}

pub struct Tree {
    tree_creator: Keypair,
    merkle_roll: Keypair,
    max_depth: u32,
    max_buffer_size: u32,
    canopy_depth: u32,
}

impl Tree {
    pub fn creator_pubkey(&self) -> Pubkey {
        self.tree_creator.pubkey()
    }

    pub fn roll_pubkey(&self) -> Pubkey {
        self.merkle_roll.pubkey()
    }

    pub fn authority(&self) -> Pubkey {
        Pubkey::find_program_address(&[self.roll_pubkey().as_ref()], &crate::id()).0
    }

    pub fn mint_authority_request(&self, authority: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[self.roll_pubkey().as_ref(), authority.as_ref()],
            &crate::id(),
        )
        .0
    }

    pub fn merkle_roll_account_size(&self) -> u64 {
        // TODO: check overflows and use u64 everywhere?
        let header_size = 8 + 32;
        let changelog_size = (self.max_depth * 32 + 32 + 4 + 4) * self.max_buffer_size;
        let rightmost_path_size = self.max_depth * 32 + 32 + 4 + 4;
        let merkle_roll_size = 8 + 8 + 16 + changelog_size + rightmost_path_size;

        // This is 0 when `canopy_depth == 0`.
        let canopy_size = ((1 << self.canopy_depth + 1) - 2) * 32;

        u64::from(merkle_roll_size + header_size + canopy_size)
    }

    pub fn alloc_instruction(&self, rent: Rent, payer: Pubkey) -> Instruction {
        let account_size = self.merkle_roll_account_size();

        // u64 -> usize conversion should never fail on the platforms we're running on.
        let lamports = rent.minimum_balance(usize::try_from(account_size).unwrap());

        system_instruction::create_account(
            &payer,
            &self.roll_pubkey(),
            lamports,
            account_size,
            &gummyroll::id(),
        )
    }

    pub fn create_instruction(&self, payer: Pubkey) -> Instruction {
        let accounts = crate::accounts::CreateTree {
            authority: self.authority(),
            payer,
            tree_creator: self.creator_pubkey(),
            candy_wrapper: mpl_candy_wrapper::id(),
            system_program: system_program::id(),
            gummyroll_program: gummyroll::id(),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::CreateTree {
            max_depth: self.max_depth,
            max_buffer_size: self.max_buffer_size,
        };

        instruction(accounts, data)
    }

    pub fn mint_v1_instruction(
        &self,
        mint_authority: Pubkey,
        owner: Pubkey,
        delegate: Pubkey,
        message: MetadataArgs,
    ) -> Instruction {
        let accounts = crate::accounts::MintV1 {
            mint_authority,
            authority: self.authority(),
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            owner,
            delegate,
            mint_authority_request: self.mint_authority_request(&mint_authority),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::MintV1 { message };

        instruction(accounts, data)
    }

    // TODO: We want to create incorrect instructions for test; i.e. for
    // this one we'd want the creator to be some random account to see it
    // fail. Will add a `_raw` function for all instructions so we can
    // alter all parameters (something similar is needed for the tx
    // methods below as well).
    pub fn set_default_mint_request_instruction(
        &self,
        payer: Pubkey,
        mint_capacity: u64,
    ) -> Instruction {
        let tree_authority = self.authority();

        let accounts = crate::accounts::SetDefaultMintRequest {
            mint_authority_request: self.mint_authority_request(&tree_authority),
            payer,
            creator: self.creator_pubkey(),
            tree_authority,
            system_program: system_program::id(),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::CreateDefaultMintRequest { mint_capacity };

        instruction(accounts, data)
    }

    pub fn approve_mint_request_instruction(
        &self,
        mint_authority_request: Pubkey,
        tree_delegate: Pubkey,
        num_mints_to_approve: u64,
    ) -> Instruction {
        let accounts = crate::accounts::ApproveMintRequest {
            mint_authority_request,
            tree_delegate,
            tree_authority: self.authority(),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::ApproveMintAuthorityRequest {
            num_mints_to_approve,
        };

        instruction(accounts, data)
    }
}

pub struct TreeClient {
    inner: Tree,
    client: BanksClient,
}

impl std::ops::Deref for TreeClient {
    type Target = Tree;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for TreeClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl TreeClient {
    pub fn new(inner: Tree, client: BanksClient) -> Self {
        TreeClient { inner, client }
    }

    // The common code within the following functions can be refactored
    // in a helper method, but first wanted to understand what's the
    // right outer interface to have in place to best support testing.
    // Also, the methods should return `Results`s to better test error
    // conditions as well.
    pub async fn alloc(&mut self, payer: &Keypair) {
        let recent_blockhash = self.client.get_latest_blockhash().await.unwrap();
        let rent = self.client.get_rent().await.unwrap();

        self.client
            .process_transaction(Transaction::new_signed_with_payer(
                &[self.alloc_instruction(rent, payer.pubkey())],
                Some(&payer.pubkey()),
                &[payer, &self.merkle_roll],
                recent_blockhash,
            ))
            .await
            .unwrap()
    }

    pub async fn create(&mut self, payer: &Keypair) {
        let recent_blockhash = self.client.get_latest_blockhash().await.unwrap();

        self.client
            .process_transaction(Transaction::new_signed_with_payer(
                &[self.create_instruction(payer.pubkey())],
                Some(&payer.pubkey()),
                &[payer],
                recent_blockhash,
            ))
            .await
            .unwrap()
    }

    // This assumes the owner is the account paying for the tx.
    pub async fn mint_v1(
        &mut self,
        mint_authority: Pubkey,
        owner: &Keypair,
        delegate: Pubkey,
        message: MetadataArgs,
    ) {
        let recent_blockhash = self.client.get_latest_blockhash().await.unwrap();

        self.client
            .process_transaction(Transaction::new_signed_with_payer(
                &[self.mint_v1_instruction(mint_authority, owner.pubkey(), delegate, message)],
                Some(&owner.pubkey()),
                &[owner],
                recent_blockhash,
            ))
            .await
            .unwrap()
    }

    pub async fn set_default_mint_request(&mut self, payer: &Keypair, mint_capacity: u64) {
        let recent_blockhash = self.client.get_latest_blockhash().await.unwrap();

        self.client
            .process_transaction(Transaction::new_signed_with_payer(
                &[self.set_default_mint_request_instruction(payer.pubkey(), mint_capacity)],
                Some(&payer.pubkey()),
                &[payer, &self.tree_creator],
                recent_blockhash,
            ))
            .await
            .unwrap()
    }

    pub async fn approve_mint_request(
        &mut self,
        mint_authority_request: Pubkey,
        tree_delegate: &Keypair,
        num_mints_to_approve: u64,
    ) {
        let recent_blockhash = self.client.get_latest_blockhash().await.unwrap();

        self.client
            .process_transaction(Transaction::new_signed_with_payer(
                &[self.approve_mint_request_instruction(
                    mint_authority_request,
                    tree_delegate.pubkey(),
                    num_mints_to_approve,
                )],
                Some(&tree_delegate.pubkey()),
                &[tree_delegate],
                recent_blockhash,
            ))
            .await
            .unwrap()
    }

    // Returning `Option` for now; should switch to `Result` for more info
    // about the potential error conditions.
    pub async fn read_account<T>(&mut self, key: Pubkey) -> Option<T>
    where
        T: AccountDeserialize,
    {
        self.client
            .get_account(key)
            .await
            .unwrap()
            .map(|acc| T::try_deserialize(&mut acc.data.as_slice()).unwrap())
    }

    pub async fn read_tree_config(&mut self) -> Option<TreeConfig> {
        self.read_account(self.authority()).await
    }

    pub async fn read_mint_authority_request(&mut self, authority: &Pubkey) -> Option<MintRequest> {
        self.read_account(self.mint_authority_request(authority))
            .await
    }
}
