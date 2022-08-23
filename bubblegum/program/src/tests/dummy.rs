use solana_program_test::tokio;

use super::program_test;

#[tokio::test]
async fn test_dummy() {
    let context = program_test().start_with_context().await;
    assert_ne!(context.payer.to_bytes(), [0u8; 64]);
}
