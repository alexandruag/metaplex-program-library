import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  sendAndConfirmTransaction,
  SystemProgram,
  Transaction,
} from '@solana/web3.js';

import {
  getConcurrentMerkleTreeAccountSize,
  createVerifyLeafIx,
  ConcurrentMerkleTreeAccount,
  SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
  SPL_NOOP_PROGRAM_ID,
} from '@solana/spl-account-compression';

import {
    createCreateTreeInstruction,
    createMintV1Instruction,
    createVerifyCreatorInstruction,
    MetadataArgs,
    PROGRAM_ID as BUBBLEGUM_PROGRAM_ID,
    TokenProgramVersion,
    TokenStandard,
    Creator,
    VerifyCreatorInstructionArgs
} from '../src/generated'
import {
    getLeafAssetId,
    computeCompressedNFTHash,
    computeDataHash
} from '../src/mpl-bubblegum';
import { BN } from 'bn.js';

import { keccak_256 } from 'js-sha3'

function keypairFromSeed(seed: string) {
  const expandedSeed = Uint8Array.from(
    Buffer.from(`${seed}                                           `),
  );
  return Keypair.fromSeed(expandedSeed.slice(0, 32));
}

function bufferToArray(buffer: Buffer): number[] {
    const nums = [];
    for (let i = 0; i < buffer.length; i++) {
      nums.push(buffer[i]);
    }
    return nums;
}

function computeCreatorHash2(creators: Creator[]) {
    let bufferOfCreatorData = Buffer.from([]);
    let bufferOfCreatorShares = Buffer.from([]);
    for (let creator of creators) {
      bufferOfCreatorData = Buffer.concat([
        bufferOfCreatorData,
        creator.address.toBuffer(),
        Buffer.from([+creator.verified]),
        Buffer.from([creator.share]),
      ]);
      bufferOfCreatorShares = Buffer.concat([
        bufferOfCreatorShares,
        Buffer.from([creator.share]),
      ]);
    }
    let creatorHash = bufferToArray(
      Buffer.from(keccak_256.digest(bufferOfCreatorData))
    );
    return creatorHash;
  }

function makeCompressedNFT(name: string, symbol: string, creators: Creator[] = []): MetadataArgs {
  return {
    name: name,
    symbol: symbol,
    uri: 'https://metaplex.com',
    creators,
    editionNonce: 0,
    tokenProgramVersion: TokenProgramVersion.Original,
    tokenStandard: TokenStandard.Fungible,
    uses: null,
    collection: null,
    primarySaleHappened: false,
    sellerFeeBasisPoints: 0,
    isMutable: false,
  };
}

async function setupTreeWithCompressedNFT(
    connection: Connection,
    payerKeypair: Keypair,
    compressedNFT: MetadataArgs,
    maxDepth: number = 14,
    maxBufferSize: number = 64,
): Promise<{
  merkleTree: PublicKey;
}> {
  const payer = payerKeypair.publicKey;

  const merkleTreeKeypair = Keypair.generate();
  const merkleTree = merkleTreeKeypair.publicKey;
  const space = getConcurrentMerkleTreeAccountSize(maxDepth, maxBufferSize);
  const allocTreeIx = SystemProgram.createAccount({
    fromPubkey: payer,
    newAccountPubkey: merkleTree,
    lamports: await connection.getMinimumBalanceForRentExemption(space),
    space: space,
    programId: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
  });
  const [treeAuthority, _bump] = await PublicKey.findProgramAddress(
    [merkleTree.toBuffer()],
    BUBBLEGUM_PROGRAM_ID,
  );
  const createTreeIx = createCreateTreeInstruction(
    {
      merkleTree,
      treeAuthority,
      treeCreator: payer,
      payer,
      logWrapper: SPL_NOOP_PROGRAM_ID,
      compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
    },
    {
      maxBufferSize,
      maxDepth,
    },
    BUBBLEGUM_PROGRAM_ID,
  );

    const mintIx = createMintV1Instruction(
        {
            merkleTree,
            treeAuthority,
            treeDelegate: payer,
            payer,
            leafDelegate: payer,
            leafOwner: payer,
            compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
            logWrapper: SPL_NOOP_PROGRAM_ID,
        },
        {
            message: compressedNFT
        }
    );

    let tx = new Transaction().add(allocTreeIx).add(createTreeIx).add(mintIx);
    tx.feePayer = payer;
    await sendAndConfirmTransaction(connection, tx, [merkleTreeKeypair, payerKeypair], {
        commitment: "confirmed",
        skipPreflight: true
    })

    if (compressedNFT.creators.length > 0) {
        const accountInfo = await connection.getAccountInfo(merkleTree, { commitment: "confirmed" });
        const account = ConcurrentMerkleTreeAccount.fromBuffer(accountInfo!.data!);

        compressedNFT.creators[0].verified = true;

        const verifyArgs :VerifyCreatorInstructionArgs = {
            root: bufferToArray(account.getCurrentRoot()),
            dataHash: bufferToArray(computeDataHash(compressedNFT)),
            creatorHash: computeCreatorHash2(compressedNFT.creators),
            nonce: new BN.BN(0),
            index: 0,
            message: compressedNFT
        };

        const verifyIx = createVerifyCreatorInstruction(
            {
                treeAuthority,
                leafOwner: payer,
                leafDelegate: payer,
                merkleTree,
                payer,
                creator: payer,
                logWrapper: SPL_NOOP_PROGRAM_ID,
                compressionProgram: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
            },
            verifyArgs
        );

        let tx2 = new Transaction().add(verifyIx);
        tx2.feePayer = payer;
        await sendAndConfirmTransaction(connection, tx2, [payerKeypair], {
            commitment: "confirmed",
            skipPreflight: true
        })
    }

  return {
    merkleTree,
  };
}

describe("Bubblegum tests", () => {
    const connection = new Connection("http://localhost:8899");
    const payerKeypair = keypairFromSeed("metaplex-test");
    const payer = payerKeypair.publicKey;

    beforeEach(async () => {
        await connection.requestAirdrop(payer, LAMPORTS_PER_SOL);
    })
    it("Can create a Bubblegum tree and mint to it", async () => {

        var creator: Creator = {
            address: payer,
            verified: true,
            share: 100,
        };

        var compressedNFT: MetadataArgs = {
            name: "Test Compressed NFT",
            symbol: "TST",
            uri: "https://metaplex.com",
            creators: [creator],
            editionNonce: 0,
            tokenProgramVersion: TokenProgramVersion.Original,
            tokenStandard: TokenStandard.Fungible,
            uses: null,
            collection: null,
            primarySaleHappened: false,
            sellerFeeBasisPoints: 0,
            isMutable: false,
        };
        await setupTreeWithCompressedNFT(connection, payerKeypair, compressedNFT, 14, 64);
    })

  describe('Unit test compressed NFT instructions', () => {
    let merkleTree: PublicKey;
    const originalCompressedNFT = makeCompressedNFT('test', 'TST');
    beforeEach(async () => {
      await connection.requestAirdrop(payer, LAMPORTS_PER_SOL);
      const result = await setupTreeWithCompressedNFT(
        connection,
        payerKeypair,
        originalCompressedNFT,
        14,
        64,
      );
      merkleTree = result.merkleTree;
    });
    it('Can verify existence a compressed NFT', async () => {
      // Todo(@ngundotra): expose commitment level in ConcurrentMerkleTreeAccount.fromAddress
      const accountInfo = await connection.getAccountInfo(merkleTree, { commitment: 'confirmed' });
      const account = ConcurrentMerkleTreeAccount.fromBuffer(accountInfo!.data!);

      // Verify leaf exists
      const leafIndex = new BN.BN(0);
      const assetId = await getLeafAssetId(merkleTree, leafIndex);
      const verifyLeafIx = createVerifyLeafIx(
        merkleTree,
        account.getCurrentRoot(),
        computeCompressedNFTHash(assetId, payer, payer, leafIndex, originalCompressedNFT),
        0,
        [],
      );
      const tx = new Transaction().add(verifyLeafIx);
      const txId = await sendAndConfirmTransaction(connection, tx, [payerKeypair], {
        commitment: 'confirmed',
        skipPreflight: true,
      });
      console.log('Verified NFT existence:', txId);
    });

    // TODO(@metaplex): add collection tests here
  });
});
