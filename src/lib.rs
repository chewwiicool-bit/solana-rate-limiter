use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint,
    entrypoint::ProgramResult,
    msg,
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction,
    system_program,
    sysvar::Sysvar,
    program::invoke_signed,
};

// ─── State ───────────────────────────────────────────────────────────────────

/// Global config PDA  seeds: ["config"]
#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct Config {
    pub authority: Pubkey,
    pub default_max_calls: u64,
    pub default_window_secs: u64,
    pub bump: u8,
}

impl Config {
    pub const SIZE: usize = 32 + 8 + 8 + 1 + 8; // +8 discriminator
}

/// Per-service config PDA  seeds: ["service", service_id]
#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct Service {
    pub service_id: [u8; 32],
    pub max_calls: u64,
    pub window_secs: u64,
    pub active: bool,
    pub created_at: i64,
    pub bump: u8,
}

impl Service {
    pub const SIZE: usize = 32 + 8 + 8 + 1 + 8 + 1 + 8; // +8 discriminator
}

/// Per-client per-service record PDA  seeds: ["client", service_id, client]
#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct ClientRecord {
    pub service_id: [u8; 32],
    pub client: Pubkey,
    pub call_count: u64,
    pub window_start: i64,
    pub last_call: i64,
    pub bump: u8,
}

impl ClientRecord {
    pub const SIZE: usize = 32 + 32 + 8 + 8 + 8 + 1 + 8; // +8 discriminator
}

// ─── Instructions ─────────────────────────────────────────────────────────────

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub enum RateLimiterInstruction {
    /// Initialize the global Config PDA
    /// Accounts: [authority(signer), config_pda(writable), system_program]
    Initialize {
        default_max_calls: u64,
        default_window_secs: u64,
    },

    /// Register a new service
    /// Accounts: [authority(signer), config_pda, service_pda(writable), system_program]
    RegisterService {
        service_id: String,
        max_calls: u64,
        window_secs: u64,
    },

    /// Check rate limit for caller — creates or updates ClientRecord
    /// Accounts: [client(signer), service_pda, client_record_pda(writable), system_program, sysvar_clock]
    CheckRateLimit { service_id: String },

    /// Admin reset a specific client
    /// Accounts: [authority(signer), config_pda, service_pda, client_record_pda(writable)]
    ResetClient { service_id: String, client: Pubkey },

    /// Update service params
    /// Accounts: [authority(signer), config_pda, service_pda(writable)]
    UpdateService {
        service_id: String,
        max_calls: u64,
        window_secs: u64,
        active: bool,
    },
}

// ─── Custom Errors ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum RateLimiterError {
    RateLimitExceeded = 0,
    ServiceNotFound   = 1,
    ServiceInactive   = 2,
    Unauthorized      = 3,
    AlreadyInitialized= 4,
}

impl From<RateLimiterError> for ProgramError {
    fn from(e: RateLimiterError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn service_id_bytes(s: &str) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let b = s.as_bytes();
    let len = b.len().min(32);
    buf[..len].copy_from_slice(&b[..len]);
    buf
}

fn create_or_realloc<'a>(
    pda: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    size: usize,
    seeds: &[&[u8]],
    program_id: &Pubkey,
) -> ProgramResult {
    if pda.data_len() > 0 {
        return Ok(()); // already exists
    }
    let rent = Rent::get()?;
    let lamports = rent.minimum_balance(size);
    invoke_signed(
        &system_instruction::create_account(payer.key, pda.key, lamports, size as u64, program_id),
        &[payer.clone(), pda.clone(), system.clone()],
        &[seeds],
    )
}

// ─── Entrypoint ───────────────────────────────────────────────────────────────

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = RateLimiterInstruction::try_from_slice(instruction_data)
        .map_err(|_| ProgramError::InvalidInstructionData)?;

    match ix {
        RateLimiterInstruction::Initialize { default_max_calls, default_window_secs } => {
            process_initialize(program_id, accounts, default_max_calls, default_window_secs)
        }
        RateLimiterInstruction::RegisterService { service_id, max_calls, window_secs } => {
            process_register_service(program_id, accounts, &service_id, max_calls, window_secs)
        }
        RateLimiterInstruction::CheckRateLimit { service_id } => {
            process_check_rate_limit(program_id, accounts, &service_id)
        }
        RateLimiterInstruction::ResetClient { service_id, client } => {
            process_reset_client(program_id, accounts, &service_id, &client)
        }
        RateLimiterInstruction::UpdateService { service_id, max_calls, window_secs, active } => {
            process_update_service(program_id, accounts, &service_id, max_calls, window_secs, active)
        }
    }
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

fn process_initialize(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    default_max_calls: u64,
    default_window_secs: u64,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority = next_account_info(iter)?;
    let config_pda = next_account_info(iter)?;
    let system_prog = next_account_info(iter)?;

    if !authority.is_signer { return Err(RateLimiterError::Unauthorized.into()); }

    let (expected_pda, bump) = Pubkey::find_program_address(&[b"config"], program_id);
    if *config_pda.key != expected_pda { return Err(ProgramError::InvalidArgument); }

    create_or_realloc(config_pda, authority, system_prog, Config::SIZE, &[b"config", &[bump]], program_id)?;

    let config = Config { authority: *authority.key, default_max_calls, default_window_secs, bump };
    config.serialize(&mut &mut config_pda.data.borrow_mut()[..])?;
    msg!("Rate Limiter initialized. max_calls={} window={}s", default_max_calls, default_window_secs);
    Ok(())
}

fn process_register_service(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    service_id: &str,
    max_calls: u64,
    window_secs: u64,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority = next_account_info(iter)?;
    let config_pda = next_account_info(iter)?;
    let service_pda = next_account_info(iter)?;
    let system_prog = next_account_info(iter)?;

    if !authority.is_signer { return Err(RateLimiterError::Unauthorized.into()); }

    let config: Config = Config::try_from_slice(&config_pda.data.borrow())?;
    if config.authority != *authority.key { return Err(RateLimiterError::Unauthorized.into()); }

    let sid = service_id_bytes(service_id);
    let (expected, bump) = Pubkey::find_program_address(&[b"service", &sid], program_id);
    if *service_pda.key != expected { return Err(ProgramError::InvalidArgument); }

    create_or_realloc(service_pda, authority, system_prog, Service::SIZE, &[b"service", &sid, &[bump]], program_id)?;

    let clock = Clock::get()?;
    let svc = Service { service_id: sid, max_calls, window_secs, active: true, created_at: clock.unix_timestamp, bump };
    svc.serialize(&mut &mut service_pda.data.borrow_mut()[..])?;
    msg!("Service '{}' registered. max_calls={} window={}s", service_id, max_calls, window_secs);
    Ok(())
}

fn process_check_rate_limit(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    service_id: &str,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let caller = next_account_info(iter)?;
    let service_pda = next_account_info(iter)?;
    let record_pda = next_account_info(iter)?;
    let system_prog = next_account_info(iter)?;

    if !caller.is_signer { return Err(RateLimiterError::Unauthorized.into()); }
    if service_pda.data_len() == 0 { return Err(RateLimiterError::ServiceNotFound.into()); }

    let svc: Service = Service::try_from_slice(&service_pda.data.borrow())?;
    if !svc.active { return Err(RateLimiterError::ServiceInactive.into()); }

    let clock = Clock::get()?;
    let now = clock.unix_timestamp;
    let sid = service_id_bytes(service_id);
    let (expected, bump) = Pubkey::find_program_address(&[b"client", &sid, caller.key.as_ref()], program_id);
    if *record_pda.key != expected { return Err(ProgramError::InvalidArgument); }

    create_or_realloc(
        record_pda, caller, system_prog, ClientRecord::SIZE,
        &[b"client", &sid, caller.key.as_ref(), &[bump]],
        program_id,
    )?;

    let mut record: ClientRecord = if record_pda.data_len() > 0 && record_pda.data.borrow()[0] != 0 {
        ClientRecord::try_from_slice(&record_pda.data.borrow())?
    } else {
        ClientRecord { service_id: sid, client: *caller.key, call_count: 0, window_start: now, last_call: now, bump }
    };

    // Reset window if expired
    if now - record.window_start >= svc.window_secs as i64 {
        record.call_count = 0;
        record.window_start = now;
    }

    if record.call_count >= svc.max_calls {
        let reset_in = svc.window_secs as i64 - (now - record.window_start);
        msg!("RATE_LIMIT_EXCEEDED: {}/{} calls. Resets in {}s", record.call_count, svc.max_calls, reset_in);
        return Err(RateLimiterError::RateLimitExceeded.into());
    }

    record.call_count += 1;
    record.last_call = now;
    record.serialize(&mut &mut record_pda.data.borrow_mut()[..])?;
    msg!("OK: {}/{} calls in window ({}s)", record.call_count, svc.max_calls, svc.window_secs);
    Ok(())
}

fn process_reset_client(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    service_id: &str,
    client: &Pubkey,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority = next_account_info(iter)?;
    let config_pda = next_account_info(iter)?;
    let _service_pda = next_account_info(iter)?;
    let record_pda = next_account_info(iter)?;

    if !authority.is_signer { return Err(RateLimiterError::Unauthorized.into()); }
    let config: Config = Config::try_from_slice(&config_pda.data.borrow())?;
    if config.authority != *authority.key { return Err(RateLimiterError::Unauthorized.into()); }

    let sid = service_id_bytes(service_id);
    let (expected, _) = Pubkey::find_program_address(&[b"client", &sid, client.as_ref()], program_id);
    if *record_pda.key != expected { return Err(ProgramError::InvalidArgument); }

    let mut record: ClientRecord = ClientRecord::try_from_slice(&record_pda.data.borrow())?;
    let clock = Clock::get()?;
    record.call_count = 0;
    record.window_start = clock.unix_timestamp;
    record.serialize(&mut &mut record_pda.data.borrow_mut()[..])?;
    msg!("Client {} reset for service '{}'", client, service_id);
    Ok(())
}

fn process_update_service(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    service_id: &str,
    max_calls: u64,
    window_secs: u64,
    active: bool,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority = next_account_info(iter)?;
    let config_pda = next_account_info(iter)?;
    let service_pda = next_account_info(iter)?;

    if !authority.is_signer { return Err(RateLimiterError::Unauthorized.into()); }
    let config: Config = Config::try_from_slice(&config_pda.data.borrow())?;
    if config.authority != *authority.key { return Err(RateLimiterError::Unauthorized.into()); }

    let sid = service_id_bytes(service_id);
    let (expected, _) = Pubkey::find_program_address(&[b"service", &sid], program_id);
    if *service_pda.key != expected { return Err(ProgramError::InvalidArgument); }

    let mut svc: Service = Service::try_from_slice(&service_pda.data.borrow())?;
    svc.max_calls = max_calls;
    svc.window_secs = window_secs;
    svc.active = active;
    svc.serialize(&mut &mut service_pda.data.borrow_mut()[..])?;
    msg!("Service '{}' updated: max={} window={}s active={}", service_id, max_calls, window_secs, active);
    Ok(())
}
