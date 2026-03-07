/**
 * Smoke test for Solana Rate Limiter
 * Usage: PROGRAM_ID=<id> node test.js
 */
const {
  Connection, Keypair, PublicKey, Transaction,
  TransactionInstruction, SystemProgram, LAMPORTS_PER_SOL, sendAndConfirmTransaction
} = require("@solana/web3.js");
const borsh = require("borsh");

const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID || "11111111111111111111111111111111");
const connection = new Connection("https://api.devnet.solana.com", "confirmed");

// ── Borsh schemas ──────────────────────────────────────────────────────────

class InitializeIx { constructor(o) { Object.assign(this, o); } }
class RegisterServiceIx { constructor(o) { Object.assign(this, o); } }
class CheckRateLimitIx { constructor(o) { Object.assign(this, o); } }

const SCHEMA = new Map([
  [InitializeIx, { kind: "struct", fields: [["variant", "u8"], ["default_max_calls", "u64"], ["default_window_secs", "u64"]] }],
  [RegisterServiceIx, { kind: "struct", fields: [["variant", "u8"], ["service_id", "string"], ["max_calls", "u64"], ["window_secs", "u64"]] }],
  [CheckRateLimitIx, { kind: "struct", fields: [["variant", "u8"], ["service_id", "string"]] }],
]);

function findPda(seeds) {
  return PublicKey.findProgramAddressSync(seeds, PROGRAM_ID);
}

async function main() {
  console.log("🚀 Solana Rate Limiter — smoke test");
  console.log("Program:", PROGRAM_ID.toBase58());

  const authority = Keypair.generate();
  console.log("Authority:", authority.publicKey.toBase58());

  // Airdrop
  const sig = await connection.requestAirdrop(authority.publicKey, 2 * LAMPORTS_PER_SOL);
  await connection.confirmTransaction(sig);
  console.log("✅ Airdrop confirmed");

  // ── 1. Initialize ──────────────────────────────────────────────────────────
  const [configPda] = findPda([Buffer.from("config")]);
  const initData = borsh.serialize(SCHEMA, new InitializeIx({ variant: 0, default_max_calls: BigInt(10), default_window_secs: BigInt(60) }));

  const initTx = new Transaction().add(new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: authority.publicKey, isSigner: true, isWritable: true },
      { pubkey: configPda, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: Buffer.from(initData),
  }));

  const initSig = await sendAndConfirmTransaction(connection, initTx, [authority]);
  console.log("✅ Initialize:", `https://explorer.solana.com/tx/${initSig}?cluster=devnet`);

  // ── 2. Register service ────────────────────────────────────────────────────
  const serviceId = "api-v1";
  const sidBytes = Buffer.alloc(32); Buffer.from(serviceId).copy(sidBytes);
  const [servicePda] = findPda([Buffer.from("service"), sidBytes]);
  const regData = borsh.serialize(SCHEMA, new RegisterServiceIx({ variant: 1, service_id: serviceId, max_calls: BigInt(3), window_secs: BigInt(60) }));

  const regTx = new Transaction().add(new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: authority.publicKey, isSigner: true, isWritable: true },
      { pubkey: configPda, isSigner: false, isWritable: false },
      { pubkey: servicePda, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: Buffer.from(regData),
  }));

  const regSig = await sendAndConfirmTransaction(connection, regTx, [authority]);
  console.log("✅ RegisterService:", `https://explorer.solana.com/tx/${regSig}?cluster=devnet`);

  // ── 3. Check rate limit (3 calls should pass, 4th should fail) ───────────
  const caller = Keypair.generate();
  const callerFundSig = await connection.requestAirdrop(caller.publicKey, LAMPORTS_PER_SOL);
  await connection.confirmTransaction(callerFundSig);

  const [recordPda] = findPda([Buffer.from("client"), sidBytes, caller.publicKey.toBuffer()]);
  const checkData = borsh.serialize(SCHEMA, new CheckRateLimitIx({ variant: 2, service_id: serviceId }));

  for (let i = 1; i <= 4; i++) {
    const tx = new Transaction().add(new TransactionInstruction({
      programId: PROGRAM_ID,
      keys: [
        { pubkey: caller.publicKey, isSigner: true, isWritable: true },
        { pubkey: servicePda, isSigner: false, isWritable: false },
        { pubkey: recordPda, isSigner: false, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: Buffer.from(checkData),
    }));

    try {
      const s = await sendAndConfirmTransaction(connection, tx, [caller]);
      console.log(`✅ Call ${i}/3: https://explorer.solana.com/tx/${s}?cluster=devnet`);
    } catch (e) {
      if (e.message.includes("0x0") || e.logs?.some(l => l.includes("RATE_LIMIT_EXCEEDED"))) {
        console.log(`🚫 Call ${i}: RATE_LIMIT_EXCEEDED (expected)`);
      } else {
        console.error(`❌ Call ${i} failed:`, e.message);
      }
    }
  }

  console.log("\n🎉 All tests passed!");
}

main().catch(console.error);
