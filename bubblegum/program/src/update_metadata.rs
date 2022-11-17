//! This module is temporarily added to hold local code use to validate the input
// of the leaf metadata update operation. For the med-to-long term, we plan to
// refactor things and reuse the same logic for this operation and the equivalent
// from Token Metadata.

use std::collections::HashMap;

use anchor_lang::{prelude::*, solana_program::pubkey::Pubkey};

use crate::error::BubblegumError;
use crate::state::metaplex_adapter::{Collection, Creator, MetadataArgs, UseMethod, Uses};

// Copied from Token Metadata, but altered. The body keeps the removed parts as commented,
// so the differences are easier to identify.
pub fn process_update_metadata_accounts_v2(
    new: &MetadataArgs,
    old: &MetadataArgs,
    update_authority: &Pubkey,
) -> Result<()> {
    //let account_info_iter = &mut accounts.iter();

    //let metadata_account_info = next_account_info(account_info_iter)?;
    //let update_authority_info = next_account_info(account_info_iter)?;
    //let mut metadata = Metadata::from_account_info(metadata_account_info)?;

    //assert_owned_by(metadata_account_info, program_id)?;
    //assert_update_authority_is_correct(&metadata, update_authority_info)?;

    //if let Some(data) = optional_data {
    if old.is_mutable {
        // let compatible_data = data.to_v1();
        assert_data_valid(
            new,
            update_authority,
            old,
            false,
            true, //update_authority_info.is_signer,
        )?;

        // The assignments are no longer needed as they are implicitly enacted when
        // we update the leaf to the new hash in Bubblegum.
        // metadata.data = compatible_data;

        // If the user passes in Collection data, only allow updating if it's unverified
        // or if it exactly matches the existing collection info.
        // If the user passes in None for the Collection data then only set it if it's unverified.
        if new.collection.is_some() {
            assert_collection_update_is_valid(false, &old.collection, &new.collection)?;
            // metadata.collection = data.collection;
        } else if let Some(collection) = old.collection.as_ref() {
            // Can't change a verified collection in this command.
            if collection.verified {
                return Err(BubblegumError::CannotUpdateVerifiedCollection.into());
            }
            // If it's unverified, it's ok to set to None.
            // metadata.collection = data.collection;
        }
        // If already None leave it as None.
        assert_valid_use(&new.uses, &old.uses)?;
        // metadata.uses = data.uses;
    } else {
        return Err(BubblegumError::DataIsImmutable.into());
    }
    //}

    // if let Some(val) = update_authority {
    //     metadata.update_authority = val;
    // }

    // if let Some(val) = primary_sale_happened {
    {
        let val = new.primary_sale_happened;
        // If received val is true, flip to true.
        if val || !old.primary_sale_happened {
            // metadata.primary_sale_happened = val
        } else {
            return Err(BubblegumError::PrimarySaleCanOnlyBeFlippedToTrue.into());
        }
    }
    // }

    // if let Some(val) = is_mutable {
    {
        let val = new.is_mutable;

        // If received value is false, flip to false.
        if !val || old.is_mutable {
            // metadata.is_mutable = val
        } else {
            return Err(BubblegumError::IsMutableCanOnlyBeFlippedToFalse.into());
        }
    }
    // }

    // puff_out_data_fields(&mut metadata);
    // clean_write_metadata(&mut metadata, metadata_account_info)?;
    Ok(())
}

// We also have an `assert_metadata_is_mpl_compatible` in `utils.rs`, which captures some,
// but not all of the checks below.
fn assert_data_valid(
    new: &MetadataArgs,
    update_authority: &Pubkey,
    old: &MetadataArgs,
    allow_direct_creator_writes: bool,
    update_authority_is_signer: bool,
) -> Result<()> {
    if new.name.len() > mpl_token_metadata::state::MAX_NAME_LENGTH {
        return Err(BubblegumError::MetadataNameTooLong.into());
    }

    if new.symbol.len() > mpl_token_metadata::state::MAX_SYMBOL_LENGTH {
        return Err(BubblegumError::MetadataSymbolTooLong.into());
    }

    if new.uri.len() > mpl_token_metadata::state::MAX_URI_LENGTH {
        return Err(BubblegumError::MetadataUriTooLong.into());
    }

    if new.seller_fee_basis_points > 10000 {
        return Err(BubblegumError::MetadataBasisPointsTooHigh.into());
    }

    // Adding `Some` on the RHS to keep the code as similar to the original from TM
    // as possible. Needed since creators in bgum `MetadataArgs` is a `Vec`, whereas
    // in TM it's an `Option<Vec>` (having it a `Vec` in there as well should work,
    // with `len == 0` as the equivalent of `None`) .
    if let Some(creators) = Some(&new.creators) {
        // Should there be a `-1` here to compute the upper bound?
        if creators.len() > mpl_token_metadata::state::MAX_CREATOR_LIMIT {
            return Err(BubblegumError::CreatorsTooLong.into());
        }

        if creators.is_empty() {
            return Err(BubblegumError::NoCreatorsPresent.into());
        }

        // Store caller-supplied creator's array into a hashmap for direct lookup.
        let new_creators_map: HashMap<&Pubkey, &Creator> =
            creators.iter().map(|c| (&c.address, c)).collect();

        // Do not allow duplicate entries in the creator's array.
        if new_creators_map.len() != creators.len() {
            return Err(BubblegumError::DuplicateCreatorAddress.into());
        }

        // If there is an existing creator's array, store this in a hashmap as well.
        // Using the weird `Some` thing to add minimal changes to the TM original code.
        let existing_creators_map: Option<HashMap<&Pubkey, &Creator>> = Some(&old.creators)
            .map(|existing_creators| existing_creators.iter().map(|c| (&c.address, c)).collect());

        // Loop over new creator's map.
        let mut share_total: u8 = 0;
        for (address, creator) in &new_creators_map {
            // Add up creator shares.  After looping through all creators, will
            // verify it adds up to 100%.
            share_total = share_total
                .checked_add(creator.share)
                .ok_or(BubblegumError::NumericalOverflowError)?;

            // If this flag is set we are allowing any and all creators to be marked as verified
            // without further checking.  This can only be done in special circumstances when the
            // metadata is fully trusted such as when minting a limited edition.  Note we are still
            // checking that creator share adds up to 100%.
            if allow_direct_creator_writes {
                continue;
            }

            // If this specific creator (of this loop iteration) is a signer and an update
            // authority, then we are fine with this creator either setting or clearing its
            // own `creator.verified` flag.
            if update_authority_is_signer && **address == *update_authority {
                continue;
            }

            // If the previous two conditions are not true then we check the state in the existing
            // metadata creators array (if it exists) before allowing `creator.verified` to be set.
            if let Some(existing_creators_map) = &existing_creators_map {
                if existing_creators_map.contains_key(address) {
                    // If this specific creator (of this loop iteration) is in the existing
                    // creator's array, then it's `creator.verified` flag must match the existing
                    // state.
                    if creator.verified && !existing_creators_map[address].verified {
                        return Err(BubblegumError::CannotVerifyAnotherCreator.into());
                    } else if !creator.verified && existing_creators_map[address].verified {
                        return Err(BubblegumError::CannotUnverifyAnotherCreator.into());
                    }
                } else if creator.verified {
                    // If this specific creator is not in the existing creator's array, then we
                    // cannot set `creator.verified`.
                    return Err(BubblegumError::CannotVerifyAnotherCreator.into());
                }
            } else if creator.verified {
                // If there is no existing creators array, we cannot set `creator.verified`.
                return Err(BubblegumError::CannotVerifyAnotherCreator.into());
            }
        }

        // Ensure share total is 100%.
        if share_total != 100 {
            return Err(BubblegumError::CreatorShareTotalMustBe100.into());
        }

        // Next make sure there were not any existing creators that were already verified but not
        // listed in the new creator's array.
        if allow_direct_creator_writes {
            return Ok(());
        } else if let Some(existing_creators_map) = &existing_creators_map {
            for (address, existing_creator) in existing_creators_map {
                // If this specific existing creator (of this loop iteration is a signer and an
                // update authority, then we are fine with this creator clearing its own
                // `creator.verified` flag.
                if update_authority_is_signer && **address == *update_authority {
                    continue;
                } else if !new_creators_map.contains_key(address) && existing_creator.verified {
                    return Err(BubblegumError::CannotUnverifyAnotherCreator.into());
                }
            }
        }
    }

    Ok(())
}

// Same as the fn from Token Metadata (at the time of copying), modulo using some
// equivalent local types.
fn assert_collection_update_is_valid(
    edition: bool,
    existing: &Option<Collection>,
    incoming: &Option<Collection>,
) -> Result<()> {
    let is_incoming_verified_true = incoming.is_some() && incoming.as_ref().unwrap().verified;

    // If incoming verified is true. Confirm incoming and existing are identical
    let is_incoming_data_valid = !is_incoming_verified_true
        || (existing.is_some()
            && incoming.as_ref().unwrap().verified == existing.as_ref().unwrap().verified
            && incoming.as_ref().unwrap().key == existing.as_ref().unwrap().key);

    if !is_incoming_data_valid && !edition {
        // Never allow a collection to be verified outside of verify_collection instruction
        return Err(BubblegumError::CollectionCannotBeVerifiedInThisInstruction.into());
    }
    Ok(())
}

// Same as the fn from Token Metadata (at the time of copying), modulo using some
// equivalent local types.
fn assert_valid_use(incoming_use: &Option<Uses>, current_use: &Option<Uses>) -> Result<()> {
    if let Some(i) = incoming_use {
        if i.use_method == UseMethod::Single && (i.total != 1 || i.remaining != 1) {
            return Err(BubblegumError::InvalidUseMethod.into());
        }
        if i.use_method == UseMethod::Multiple && (i.total < 2 || i.total < i.remaining) {
            return Err(BubblegumError::InvalidUseMethod.into());
        }
    }
    match (incoming_use, current_use) {
        (Some(incoming), Some(current)) => {
            if incoming.use_method != current.use_method && current.total != current.remaining {
                return Err(BubblegumError::CannotChangeUseMethodAfterFirstUse.into());
            }
            if incoming.total != current.total && current.total != current.remaining {
                return Err(BubblegumError::CannotChangeUsesAfterFirstUse.into());
            }
            if incoming.remaining != current.remaining && current.total != current.remaining {
                return Err(BubblegumError::CannotChangeUsesAfterFirstUse.into());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
