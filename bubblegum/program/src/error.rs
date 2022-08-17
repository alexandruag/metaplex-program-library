use anchor_lang::prelude::*;

#[error_code]
pub enum BubblegumError {
    #[msg("Asset Owner Does not match")]
    AssetOwnerMismatch,
    #[msg("PublicKeyMismatch")]
    PublicKeyMismatch,
    #[msg("Hashing Mismatch Within Leaf Schema")]
    HashingMismatch,
    #[msg("Unsupported Schema Version")]
    UnsupportedSchemaVersion,
    #[msg("Creator shares must sum to 100")]
    CreatorShareTotalMustBe100,
    #[msg("No duplicate creator addresses in metadata")]
    DuplicateCreatorAddress,
    #[msg("Creators list too long")]
    CreatorsTooLong,
    #[msg("Name in metadata is too long")]
    MetadataNameTooLong,
    #[msg("Symbol in metadata is too long")]
    MetadataSymbolTooLong,
    #[msg("Uri in metadata is too long")]
    MetadataUriTooLong,
    #[msg("Basis points in metadata cannot exceed 10000")]
    MetadataBasisPointsTooHigh,
    #[msg("Not enough unapproved mints left")]
    InsufficientMintCapacity,
    #[msg("Mint request not approved")]
    MintRequestNotApproved,
    #[msg("Mint authority key does not match request")]
    MintRequestKeyMismatch,
    #[msg("Mint request data has incorrect disciminator")]
    MintRequestDiscriminatorMismatch,
    #[msg("Something went wrong closing mint request")]
    CloseMintRequestError,
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn asdf() {
//         // let err = BubblegumError::from(3009);
//         //
//         // assert_eq!(format!("{}", err), "asdf");
//
//         // let x: u32 = BubblegumError::CloseMintRequestError.into();
//         let x: u32 = BubblegumError::CloseMintRequestError.into();
//         assert_eq!(format!("{}", x), "asdf");
//     }
// }
