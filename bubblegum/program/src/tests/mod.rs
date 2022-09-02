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

fn instruction<T, U>(accounts: &T, data: &U) -> Instruction
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

pub struct TxBuilder<T, U> {
    pub accounts: T,
    pub data: U,
    pub payer: Pubkey,
    client: RefCell<BanksClient>,
    // Using only `Keypair`s as signers for now; can make this
    // more generic if needed.
    signers: Vec<Keypair>,
}

impl<T, U> TxBuilder<T, U>
where
    T: ToAccountMetas,
    U: InstructionData,
{
    fn client(&self) -> RefMut<BanksClient> {
        self.client.borrow_mut()
    }

    pub async fn execute(&self) -> Result<()> {
        let recent_blockhash = self
            .client()
            .get_latest_blockhash()
            .await
            .map_err(Error::BanksClient)?;

        let ix = instruction(&self.accounts, &self.data);

        self.client()
            .process_transaction(Transaction::new_signed_with_payer(
                &[ix],
                Some(&self.payer),
                &self.signers.iter().collect::<Vec<_>>(),
                recent_blockhash,
            ))
            .await
            .map_err(Error::BanksClient)
    }

    // Returning `&mut Self` to allow method chaining.
    pub fn set_signers(&mut self, signers: &[&Keypair]) -> &mut Self {
        self.signers = signers.iter().map(|k| clone_keypair(k)).collect();
        self
    }
}

pub type CreateBuilder = TxBuilder<crate::accounts::CreateTree, crate::instruction::CreateTree>;

pub type MintV1Builder = TxBuilder<crate::accounts::MintV1, crate::instruction::MintV1>;

pub type SetDefaultMintRequestBuilder =
    TxBuilder<crate::accounts::SetDefaultMintRequest, crate::instruction::CreateDefaultMintRequest>;

pub type ApproveMintRequestBuilder =
    TxBuilder<crate::accounts::ApproveMintRequest, crate::instruction::ApproveMintAuthorityRequest>;

pub type BurnBuilder = TxBuilder<crate::accounts::Burn, crate::instruction::Burn>;

pub type TransferBuilder = TxBuilder<crate::accounts::Transfer, crate::instruction::Transfer>;

pub type DelegateBuilder = TxBuilder<crate::accounts::Delegate, crate::instruction::Delegate>;

pub type SetTreeDelegateBuilder =
    TxBuilder<crate::accounts::SetTreeDelegate, crate::instruction::SetTreeDelegate>;

const MAX_DEPTH: u32 = 20;
const MAX_SIZE: u32 = 64;

pub struct Tree {
    pub tree_creator: Keypair,
    // TODO: Update all methods that work with the tree delegate to use this instead of a param.
    pub tree_delegate: Keypair,
    pub merkle_roll: Keypair,
    pub max_depth: u32,
    pub max_buffer_size: u32,
    pub canopy_depth: u32,
    // Using `RefCell` to provide interior mutability and circumvent some
    // annoyance with the borrow checker (i.e. provide helper methods that
    // only need &self, vs &mut self); if we'll ever need to use this
    // in a context with multiple threads, we can just replace the wrapper
    // with a `Mutex`.
    client: RefCell<BanksClient>,
}

impl Tree {
    // This and `with_creator` use a bunch of defaults; things can be
    // customized some more via the public access, or we can add extra
    // methods to make things even easier.
    pub fn new(client: BanksClient) -> Self {
        Self::with_creator(&Keypair::new(), client)
    }

    pub fn with_creator(tree_creator: &Keypair, client: BanksClient) -> Self {
        Tree {
            tree_creator: clone_keypair(tree_creator),
            tree_delegate: clone_keypair(tree_creator),
            merkle_roll: Keypair::new(),
            max_depth: MAX_DEPTH,
            max_buffer_size: MAX_SIZE,
            canopy_depth: 0,
            client: RefCell::new(client),
        }
    }

    pub fn creator_pubkey(&self) -> Pubkey {
        self.tree_creator.pubkey()
    }

    pub fn delegate_pubkey(&self) -> Pubkey {
        self.tree_delegate.pubkey()
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

    fn tx_builder<T, U>(
        &self,
        accounts: T,
        data: U,
        payer: Pubkey,
        default_signers: &[&Keypair],
    ) -> TxBuilder<T, U> {
        let def_signers = default_signers.iter().map(|k| clone_keypair(k)).collect();

        TxBuilder {
            accounts,
            data,
            payer,
            client: self.client.clone(),
            signers: def_signers,
        }
    }

    pub fn create_tx(&self, payer: &Keypair) -> CreateBuilder {
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

        self.tx_builder(accounts, data, payer.pubkey(), &[payer])
    }

    pub async fn create(&self, payer: &Keypair) -> Result<()> {
        self.create_tx(payer).execute().await
    }

    pub fn mint_v1_tx(
        &self,
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

        self.tx_builder(accounts, data, owner.pubkey(), &[owner, &self.tree_creator])
    }

    // This assumes the owner is the account paying for the tx.
    pub async fn mint_v1(
        &self,
        mint_authority: Pubkey,
        owner: &Keypair,
        delegate: Pubkey,
        message: &MetadataArgs,
    ) -> Result<()> {
        self.mint_v1_tx(mint_authority, owner, delegate, message)
            .execute()
            .await
    }

    pub fn set_default_mint_request_tx(&self, mint_capacity: u64) -> SetDefaultMintRequestBuilder {
        let tree_authority = self.authority();

        let accounts = crate::accounts::SetDefaultMintRequest {
            mint_authority_request: self.mint_authority_request(&tree_authority),
            payer: self.creator_pubkey(),
            creator: self.creator_pubkey(),
            tree_authority,
            system_program: system_program::id(),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::CreateDefaultMintRequest { mint_capacity };

        self.tx_builder(accounts, data, self.creator_pubkey(), &[&self.tree_creator])
    }

    pub async fn set_default_mint_request(&self, mint_capacity: u64) -> Result<()> {
        self.set_default_mint_request_tx(mint_capacity)
            .execute()
            .await
    }

    pub fn approve_mint_request_tx(
        &self,
        mint_authority_request: Pubkey,
        num_mints_to_approve: u64,
    ) -> ApproveMintRequestBuilder {
        let accounts = crate::accounts::ApproveMintRequest {
            mint_authority_request,
            tree_delegate: self.delegate_pubkey(),
            tree_authority: self.authority(),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::ApproveMintAuthorityRequest {
            num_mints_to_approve,
        };

        self.tx_builder(
            accounts,
            data,
            self.delegate_pubkey(),
            &[&self.tree_delegate],
        )
    }

    pub async fn approve_mint_request(
        &self,
        mint_authority_request: Pubkey,
        num_mints_to_approve: u64,
    ) -> Result<()> {
        self.approve_mint_request_tx(mint_authority_request, num_mints_to_approve)
            .execute()
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

    // Is currently async due to calling `decode_roll`. Maybe having that local tree
    // implementation could help here?
    pub async fn burn_tx(
        &self,
        owner: &Keypair,
        delegate: Pubkey,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<BurnBuilder> {
        let (root, nonce) = self.decode_roll().await?;

        let (data_hash, creator_hash) = compute_metadata_hashes(metadata_args);

        let accounts = crate::accounts::Burn {
            authority: self.authority(),
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            owner: owner.pubkey(),
            delegate,
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::Burn {
            root,
            data_hash,
            creator_hash,
            nonce,
            index,
        };

        Ok(self.tx_builder(accounts, data, owner.pubkey(), &[owner]))
    }

    pub async fn burn(
        &self,
        owner: &Keypair,
        delegate: Pubkey,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<()> {
        self.burn_tx(owner, delegate, metadata_args, index)
            .await?
            .execute()
            .await
    }

    pub async fn transfer_tx(
        &self,
        owner: &Keypair,
        delegate: Pubkey,
        new_owner: Pubkey,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<TransferBuilder> {
        let (root, nonce) = self.decode_roll().await?;
        let (data_hash, creator_hash) = compute_metadata_hashes(metadata_args);

        let accounts = crate::accounts::Transfer {
            authority: self.authority(),
            owner: self.tree_creator.pubkey(),
            delegate,
            new_owner,
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::Transfer {
            root,
            data_hash,
            creator_hash,
            nonce,
            index,
        };

        Ok(self.tx_builder(accounts, data, owner.pubkey(), &[owner]))
    }

    // Have the nft owner as a parameter
    pub async fn transfer(
        &self,
        owner: &Keypair,
        delegate: Pubkey,
        new_owner: Pubkey,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<()> {
        self.transfer_tx(owner, delegate, new_owner, metadata_args, index)
            .await?
            .execute()
            .await
    }

    pub async fn delegate_tx(
        &self,
        owner: &Keypair,
        previous_delegate: Pubkey,
        new_delegate: Pubkey,
        metadata_args: &MetadataArgs,
        index: u32,
    ) -> Result<DelegateBuilder> {
        let (root, nonce) = self.decode_roll().await?;
        let (data_hash, creator_hash) = compute_metadata_hashes(metadata_args);

        let accounts = crate::accounts::Delegate {
            authority: self.authority(),
            owner: owner.pubkey(),
            previous_delegate,
            new_delegate,
            candy_wrapper: mpl_candy_wrapper::id(),
            gummyroll_program: gummyroll::id(),
            merkle_slab: self.roll_pubkey(),
        };

        let data = crate::instruction::Delegate {
            root,
            data_hash,
            creator_hash,
            nonce,
            index,
        };

        Ok(self.tx_builder(accounts, data, owner.pubkey(), &[owner]))
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
        self.delegate_tx(owner, previous_delegate, new_delegate, metadata_args, index)
            .await?
            .execute()
            .await
    }

    pub fn set_tree_delegate_tx(&self, new_delegate: Pubkey) -> SetTreeDelegateBuilder {
        let accounts = crate::accounts::SetTreeDelegate {
            creator: self.creator_pubkey(),
            new_delegate,
            merkle_slab: self.roll_pubkey(),
            tree_authority: self.authority(),
        };

        let data = crate::instruction::SetTreeDelegate;

        self.tx_builder(accounts, data, self.creator_pubkey(), &[&self.tree_creator])
    }

    pub async fn set_tree_delegate(&mut self, new_delegate: &Keypair) -> Result<()> {
        self.set_tree_delegate_tx(new_delegate.pubkey())
            .execute()
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
