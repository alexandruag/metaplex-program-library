mod dummy;
mod simple;

use std::cell::{RefCell, RefMut};
use std::convert::TryFrom;
use std::mem::size_of;
use std::result;

use anchor_lang::{self, AccountDeserialize, InstructionData, ToAccountMetas};
use bytemuck::{try_from_bytes, PodCastError};
use gummyroll::state::MerkleRollHeader;
use gummyroll::MerkleRoll;
use solana_program::instruction::Instruction;
use solana_program::pubkey::Pubkey;
use solana_program::rent::Rent;
use solana_program::{keccak, system_instruction, system_program};
use solana_program_test::{BanksClient, BanksClientError, ProgramTest};
use solana_sdk::account::Account;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::signer::signers::Signers;
use solana_sdk::transaction::Transaction;

use crate::state::metaplex_adapter::MetadataArgs;
use crate::state::request::MintRequest;
use crate::state::TreeConfig;

#[derive(Debug)]
pub enum Error {
    Anchor(anchor_lang::error::Error),
    BanksClient(BanksClientError),
    Pod(PodCastError),
    AccountNotFound(Pubkey),
}

pub type Result<T> = result::Result<T, Error>;

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

pub struct TxBuilder<'a, T, U> {
    pub accounts: T,
    pub data: U,
    pub payer: Pubkey,
    client: RefCell<BanksClient>,
    default_signers: Vec<Keypair>,
}

impl<'a, T, U> TxBuilder<'a, T, U>
where
    T: ToAccountMetas,
    U: InstructionData,
{
    fn client(&self) -> RefMut<BanksClient> {
        self.client.borrow_mut()
    }

    pub async fn execute_with_signers<S: Signers>(mut self, signing_keypairs: S) -> Result<()> {
        let recent_blockhash = self
            .client
            .get_latest_blockhash()
            .await
            .map_err(Error::BanksClient)?;

        let ix = instruction(self.accounts, self.data);

        self.client
            .process_transaction(Transaction::new_signed_with_payer(
                &[ix],
                Some(&self.payer),
                &signing_keypairs,
                recent_blockhash,
            ))
            .await
            .map_err(Error::BanksClient)
    }

    pub async fn execute(mut self) -> Result<()> {
        let signing_keypairs: Vec<Keypair> = self.default_signers.drain(..).collect();

        self.execute_with_signers(signing_keypairs.iter().collect::<Vec<_>>())
            .await
    }
}

pub type CreateBuilder<'a> =
    TxBuilder<'a, crate::accounts::CreateTree, crate::instruction::CreateTree>;

pub type MintV1Builder<'a> = TxBuilder<'a, crate::accounts::MintV1, crate::instruction::MintV1>;

// impl CreateBuilder {
//     pub async fn execute(&mut self) -> Result<()> {
//         self.execute_with_signers()
//     }
// }

pub struct Tree {
    tree_creator: Keypair,
    // TODO: Update all methods that work with the tree delegate to use this instead of a param.
    tree_delegate: Keypair,
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
        message: &MetadataArgs,
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

        let data = crate::instruction::MintV1 {
            message: message.clone(),
        };

        instruction(accounts, data)
    }

    pub fn burn_instruction(
        &self,
        owner: Pubkey,
        delegate: Pubkey,
        metadata_args: &MetadataArgs,
        root: [u8; 32],
        nonce: u64,
        index: u32,
    ) -> Instruction {
        let accounts = crate::accounts::Burn {
            authority: self.authority(),
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            owner,
            delegate,
            merkle_slab: self.roll_pubkey(),
        };

        let (data_hash, creator_hash) = compute_metadata_hashes(metadata_args);

        let data = crate::instruction::Burn {
            root,
            data_hash,
            creator_hash,
            nonce,
            index,
        };

        instruction(accounts, data)
    }

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

    pub fn transfer_instruction(
        &self,
        delegate: Pubkey,
        new_owner: Pubkey,
        metadata_args: &MetadataArgs,
        root: [u8; 32],
        nonce: u64,
        index: u32,
    ) -> Instruction {
        let accounts = crate::accounts::Transfer {
            authority: self.authority(),
            owner: self.tree_creator.pubkey(),
            delegate,
            new_owner,
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            merkle_slab: self.roll_pubkey(),
        };

        let (data_hash, creator_hash) = compute_metadata_hashes(metadata_args);

        let data = crate::instruction::Transfer {
            root,
            data_hash,
            creator_hash,
            nonce,
            index,
        };

        instruction(accounts, data)
    }

    pub fn delegate_instruction(
        &self,
        owner: Pubkey,
        previous_delegate: Pubkey,
        new_delegate: Pubkey,
        metadata_args: &MetadataArgs,
        root: [u8; 32],
        nonce: u64,
        index: u32,
    ) -> Instruction {
        let accounts = crate::accounts::Delegate {
            authority: self.authority(),
            owner,
            previous_delegate,
            new_delegate,
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            merkle_slab: self.roll_pubkey(),
        };

        let (data_hash, creator_hash) = compute_metadata_hashes(metadata_args);

        let data = crate::instruction::Delegate {
            root,
            data_hash,
            creator_hash,
            nonce,
            index,
        };

        instruction(accounts, data)
    }

    pub fn set_tree_delegate_instruction(&self, new_delegate: Pubkey) -> Instruction {
        let accounts = crate::accounts::SetTreeDelegate {
            creator: self.creator_pubkey(),
            new_delegate,
            merkle_slab: self.roll_pubkey(),
            tree_authority: self.authority(),
        };

        let data = crate::instruction::SetTreeDelegate;

        instruction(accounts, data)
    }
}

// Computes the `data_hash` and `creator_hash`. Taken from the contract code where something
// similar is computed. Needs cleanup.
fn compute_metadata_hashes(metadata_args: &MetadataArgs) -> ([u8; 32], [u8; 32]) {
    let data_hash = crate::hash_metadata(metadata_args).expect("handle error?");

    let creator_data = metadata_args
        .creators
        .iter()
        .map(|c| {
            // if c.verified && !metadata_auth.contains(&c.address) {
            //     panic!("aaaaaaa");
            // } else {
            [c.address.as_ref(), &[c.verified as u8], &[c.share]].concat()
            //}
        })
        .collect::<Vec<_>>();

    // Calculate creator hash.
    let creator_hash = keccak::hashv(
        creator_data
            .iter()
            .map(|c| c.as_slice())
            .collect::<Vec<&[u8]>>()
            .as_ref(),
    )
    .0;

    (data_hash, creator_hash)
}

pub struct TreeClient {
    inner: Tree,
    // Using `RefCell` to provide interior mutability and circumvent some
    // annoyance with the borrow checker (i.e. provide helper methods that
    // only need &self, vs &mut self); if we'll ever need to use this
    // in a context with multiple threads, we can just replace the wrapper
    // with a `Mutex`.
    client: RefCell<BanksClient>,
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
        TreeClient {
            inner,
            client: RefCell::new(client.clone()),
        }
    }

    pub fn client(&self) -> RefMut<BanksClient> {
        self.client.borrow_mut()
    }

    pub async fn process_tx<T: Signers>(
        &self,
        instruction: Instruction,
        payer: &Pubkey,
        signing_keypairs: &T,
    ) -> Result<()> {
        let recent_blockhash = self
            .client()
            .get_latest_blockhash()
            .await
            .map_err(Error::BanksClient)?;

        self.client()
            .process_transaction(Transaction::new_signed_with_payer(
                &[instruction],
                Some(payer),
                signing_keypairs,
                recent_blockhash,
            ))
            .await
            .map_err(Error::BanksClient)
    }

    pub async fn rent(&self) -> Result<Rent> {
        self.client().get_rent().await.map_err(Error::BanksClient)
    }

    // The common code within the following functions can be refactored
    // in a helper method, but first wanted to understand what's the
    // right outer interface to have in place to best support testing.
    // Also, the methods should return `Results`s to better test error
    // conditions as well.
    pub async fn alloc(&self, payer: &Keypair) -> Result<()> {
        let rent = self.rent().await?;
        self.process_tx(
            self.alloc_instruction(rent, payer.pubkey()),
            &payer.pubkey(),
            &[payer, &self.merkle_roll],
        )
        .await
    }

    pub fn create_tx(&mut self, payer: &Keypair) -> CreateBuilder {
        let accounts = crate::accounts::CreateTree {
            authority: self.authority(),
            payer: payer.pubkey(),
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

        let client = self.client.borrow().clone();

        CreateBuilder {
            accounts,
            data,
            payer: payer.pubkey(),
            tree: self,
            client,
            default_signers: vec![clone_keypair(payer)],
        }
    }

    pub async fn create(&mut self, payer: &Keypair) -> Result<()> {
        self.create_tx(payer).execute().await
    }

    pub fn mint_v1_tx(
        &mut self,
        mint_authority: Pubkey,
        owner: &Keypair,
        delegate: Pubkey,
        message: &MetadataArgs,
    ) -> MintV1Builder {
        let accounts = crate::accounts::MintV1 {
            mint_authority,
            authority: self.authority(),
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            owner: owner.pubkey(),
            delegate,
            mint_authority_request: self.mint_authority_request(&mint_authority),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::MintV1 {
            message: message.clone(),
        };

        let client = self.client.borrow().clone();

        MintV1Builder {
            accounts,
            data,
            payer: owner.pubkey(),
            tree: self,
            client,
            default_signers: vec![clone_keypair(owner), clone_keypair(&self.tree_creator)],
        }
    }

    // This assumes the owner is the account paying for the tx.
    pub async fn mint_v1(
        &mut self,
        mint_authority: Pubkey,
        owner: &Keypair,
        delegate: Pubkey,
        message: &MetadataArgs,
    ) -> Result<()> {
        // self.process_tx(
        //     self.mint_v1_instruction(mint_authority, owner.pubkey(), delegate, message),
        //     &owner.pubkey(),
        //     &[owner],
        // )
        // .await

        self.mint_v1_tx(mint_authority, owner, delegate, message).execute().await
    }

    pub async fn set_default_mint_request(
        &self,
        payer: &Keypair,
        mint_capacity: u64,
    ) -> Result<()> {
        self.process_tx(
            self.set_default_mint_request_instruction(payer.pubkey(), mint_capacity),
            &payer.pubkey(),
            &[payer, &self.tree_creator],
        )
        .await
    }

    pub async fn approve_mint_request(
        &self,
        mint_authority_request: Pubkey,
        tree_delegate: &Keypair,
        num_mints_to_approve: u64,
    ) -> Result<()> {
        self.process_tx(
            self.approve_mint_request_instruction(
                mint_authority_request,
                tree_delegate.pubkey(),
                num_mints_to_approve,
            ),
            &tree_delegate.pubkey(),
            &[tree_delegate],
        )
        .await
    }

    // When `Ok`, returns a (root, nonce) pair. Might need a better implementation for this
    // function, for example because it assumes a certain MAX_DEPTH and MAX_BUFFER_SIZE
    // for the roll (i.e. 20 & 64).
    pub async fn decode_roll(&self) -> Result<([u8; 32], u64)> {
        let mut roll_account = self.read_account(self.roll_pubkey()).await?;

        let merkle_roll_bytes = roll_account.data.as_mut_slice();
        let (_header_bytes, rest) = merkle_roll_bytes.split_at_mut(size_of::<MerkleRollHeader>());
        // Using the merkle_roll_get_size! from gummyroll here requires a bunch of visibility
        // changes and exports in there. Hardcoding for now. (Or maybe should just pass them
        // as generic method params if that make sense as a crutch).
        let merkle_roll_size = size_of::<MerkleRoll<20, 64>>();
        let roll_bytes = &mut rest[..merkle_roll_size];

        let roll = try_from_bytes::<MerkleRoll<20, 64>>(roll_bytes).map_err(Error::Pod)?;

        // println!("roll active index {}", roll.active_index);

        let root = roll.change_logs[roll.active_index as usize].root;

        // println!("root {:?}", root);

        // Is the nonce always `num_minted - 1`, or `num_minted - 1` when the asset
        // got minted ?! prob the latter?
        let nonce = self.read_tree_config().await?.num_minted - 1;

        // println!("nonce {:?}", nonce);

        Ok((root, nonce))
    }

    pub async fn burn(
        &self,
        owner: &Keypair,
        delegate: Pubkey,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<()> {
        let (root, nonce) = self.decode_roll().await?;

        self.process_tx(
            self.burn_instruction(owner.pubkey(), delegate, metadata_args, root, nonce, index),
            &owner.pubkey(),
            &[owner],
        )
        .await
    }

    // Have the nft owner as a parameter
    pub async fn transfer(
        &self,
        delegate: Pubkey,
        // Pubkey here right?
        new_owner: &Keypair,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<()> {
        let (root, nonce) = self.decode_roll().await?;

        self.process_tx(
            self.transfer_instruction(
                delegate,
                new_owner.pubkey(),
                metadata_args,
                root,
                nonce,
                index,
            ),
            &self.creator_pubkey(),
            &[&self.tree_creator],
        )
        .await
    }

    // Does the prev delegate need to sign as well?
    pub async fn delegate(
        &self,
        owner: &Keypair,
        previous_delegate: Pubkey,
        new_delegate: Pubkey,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<()> {
        let (root, nonce) = self.decode_roll().await?;

        self.process_tx(
            self.delegate_instruction(
                owner.pubkey(),
                previous_delegate,
                new_delegate,
                metadata_args,
                root,
                nonce,
                index,
            ),
            &owner.pubkey(),
            &[owner],
        )
        .await
    }

    pub async fn set_tree_delegate(&mut self, new_delegate: &Keypair) -> Result<()> {
        self.process_tx(
            self.set_tree_delegate_instruction(new_delegate.pubkey()),
            &self.creator_pubkey(),
            &[&self.tree_creator],
        )
        .await?;

        self.tree_delegate = clone_keypair(new_delegate);

        Ok(())
    }

    // Move to another struct?
    async fn read_account(&self, key: Pubkey) -> Result<Account> {
        self.client()
            .get_account(key)
            .await
            .map_err(Error::BanksClient)?
            .ok_or(Error::AccountNotFound(key))
    }

    // Returning `Option` for now; should switch to `Result` for more info
    // about the potential error conditions.
    pub async fn read_account_data<T>(&self, key: Pubkey) -> Result<T>
    where
        T: AccountDeserialize,
    {
        self.read_account(key)
            .await
            .and_then(|acc| T::try_deserialize(&mut acc.data.as_slice()).map_err(Error::Anchor))
    }

    pub async fn read_tree_config(&self) -> Result<TreeConfig> {
        self.read_account_data(self.authority()).await
    }

    pub async fn read_mint_authority_request(&self, authority: &Pubkey) -> Result<MintRequest> {
        self.read_account_data(self.mint_authority_request(authority))
            .await
    }
}
