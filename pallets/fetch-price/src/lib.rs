#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode};
use frame_support::{
    debug, decl_event, decl_module, decl_storage, dispatch::DispatchResult, traits::Get,
};
use frame_system::{
    self as system, ensure_none, ensure_signed,
    offchain::{
        AppCrypto, CreateSignedTransaction, SendSignedTransaction, SendUnsignedTransaction,
        SignedPayload, Signer, SigningTypes, SubmitTransaction,
    },
};
use lite_json::json::JsonValue;
use sp_core::crypto::KeyTypeId;
use sp_runtime::{
    offchain::{http, storage::StorageValueRef, Duration},
    traits::Zero,
    transaction_validity::{
        InvalidTransaction, TransactionPriority, TransactionSource, TransactionValidity,
        ValidTransaction,
    },
    RuntimeDebug,
};
use sp_std::vec::Vec;

pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"usd!");

pub mod crypto {
    use super::KEY_TYPE;
    use sp_core::sr25519::Signature as Sr25519Signature;
    use sp_runtime::{
        app_crypto::{app_crypto, sr25519},
        traits::Verify,
    };
    app_crypto!(sr25519, KEY_TYPE);

    pub struct TestAuthId;
    impl frame_system::offchain::AppCrypto<<Sr25519Signature as Verify>::Signer, Sr25519Signature>
        for TestAuthId
    {
        type RuntimeAppPublic = Public;
        type GenericSignature = sp_core::sr25519::Signature;
        type GenericPublic = sp_core::sr25519::Public;
    }
}

/// This pallet's configuration trait
pub trait Trait: CreateSignedTransaction<Call<Self>> {
    /// The identifier type for an offchain worker.
    type AuthorityId: AppCrypto<Self::Public, Self::Signature>;

    /// The overarching event type.
    type Event: From<Event<Self>> + Into<<Self as frame_system::Trait>::Event>;
    /// The overarching dispatch call type.
    type Call: From<Call<Self>>;

    type GracePeriod: Get<Self::BlockNumber>;

    type UnsignedInterval: Get<Self::BlockNumber>;

    type UnsignedPriority: Get<TransactionPriority>;
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct PricePayload<Public, BlockNumber> {
    block_number: BlockNumber,
    price: u32,
    public: Public,
}

impl<T: SigningTypes> SignedPayload<T> for PricePayload<T::Public, T::BlockNumber> {
    fn public(&self) -> T::Public {
        self.public.clone()
    }
}

decl_storage! {
    trait Store for Module<T: Trait> as ExampleOffchainWorker {

        Prices get(fn prices): Vec<u32>;
        NextUnsignedAt get(fn next_unsigned_at): T::BlockNumber;
    }
}

decl_event!(
    /// Events generated by the module.
    pub enum Event<T>
    where
        AccountId = <T as frame_system::Trait>::AccountId,
    {
        NewPrice(u32, AccountId),
    }
);

decl_module! {
    /// A public part of the pallet.
    pub struct Module<T: Trait> for enum Call where origin: T::Origin {
        fn deposit_event() = default;


        #[weight = 0]
        pub fn submit_price(origin, price: u32) -> DispatchResult {
            // Retrieve sender of the transaction.
            let who = ensure_signed(origin)?;
            // Add the price to the on-chain list.
            Self::add_price(who, price);
            Ok(())
        }


        #[weight = 0]
        pub fn submit_price_unsigned(origin, _block_number: T::BlockNumber, price: u32)
            -> DispatchResult
        {
            // This ensures that the function can only be called via unsigned transaction.
            ensure_none(origin)?;
            // Add the price to the on-chain list, but mark it as coming from an empty address.
            Self::add_price(Default::default(), price);
            // now increment the block number at which we expect next unsigned transaction.
            let current_block = <system::Module<T>>::block_number();
            <NextUnsignedAt<T>>::put(current_block + T::UnsignedInterval::get());
            Ok(())
        }

        #[weight = 0]
        pub fn submit_price_unsigned_with_signed_payload(
            origin,
            price_payload: PricePayload<T::Public, T::BlockNumber>,
            _signature: T::Signature,
        ) -> DispatchResult {
            // This ensures that the function can only be called via unsigned transaction.
            ensure_none(origin)?;
            // Add the price to the on-chain list, but mark it as coming from an empty address.
            Self::add_price(Default::default(), price_payload.price);
            // now increment the block number at which we expect next unsigned transaction.
            let current_block = <system::Module<T>>::block_number();
            <NextUnsignedAt<T>>::put(current_block + T::UnsignedInterval::get());
            Ok(())
        }


        fn offchain_worker(block_number: T::BlockNumber) {

            debug::native::info!("Hello World from offchain workers!");


            let parent_hash = <system::Module<T>>::block_hash(block_number - 1u32.into());
            debug::debug!("Current block: {:?} (parent hash: {:?})", block_number, parent_hash);


            let average: Option<u32> = Self::average_price();
            debug::debug!("Current price: {:?}", average);


            let should_send = Self::choose_transaction_type(block_number);
            let res = match should_send {
                TransactionType::Signed => Self::fetch_price_and_send_signed(),
                TransactionType::UnsignedForAny => Self::fetch_price_and_send_unsigned_for_any_account(block_number),
                TransactionType::UnsignedForAll => Self::fetch_price_and_send_unsigned_for_all_accounts(block_number),
                TransactionType::Raw => Self::fetch_price_and_send_raw_unsigned(block_number),
                TransactionType::None => Ok(()),
            };
            if let Err(e) = res {
                debug::error!("Error: {}", e);
            }
        }
    }
}

enum TransactionType {
    Signed,
    UnsignedForAny,
    UnsignedForAll,
    Raw,
    None,
}
pub trait FetchPriceFor {
    fn fetch_price() -> Result<u32, http::Error>;
}

impl<T: Trait> FetchPriceFor for Module<T> {
    /// Fetch current price and return the result in cents.
    fn fetch_price() -> Result<u32, http::Error> {
        let deadline = sp_io::offchain::timestamp().add(Duration::from_millis(2_000));
 
        let request =
            http::Request::get("https://min-api.cryptocompare.com/data/price?fsym=BTC&tsyms=USD");

        let pending = request
            .deadline(deadline)
            .send()
            .map_err(|_| http::Error::IoError)?;

        let response = pending
            .try_wait(deadline)
            .map_err(|_| http::Error::DeadlineReached)??;
        // Let's check the status code before we proceed to reading the response.
        if response.code != 200 {
            debug::warn!("Unexpected status code: {}", response.code);
            return Err(http::Error::Unknown);
        }

        let body = response.body().collect::<Vec<u8>>();

        // Create a str slice from the body.
        let body_str = sp_std::str::from_utf8(&body).map_err(|_| {
            debug::warn!("No UTF8 body");
            http::Error::Unknown
        })?;

        let price = match Self::parse_price(body_str) {
            Some(price) => Ok(price),
            None => {
                debug::warn!("Unable to extract price from the response: {:?}", body_str);
                Err(http::Error::Unknown)
            }
        }?;

        debug::warn!("Got price: {} cents", price);

        Ok(price)
    }
}

impl<T: Trait> Module<T> {
    fn choose_transaction_type(block_number: T::BlockNumber) -> TransactionType {
        const RECENTLY_SENT: () = ();

        let val = StorageValueRef::persistent(b"example_ocw::last_send");

        let res = val.mutate(|last_send: Option<Option<T::BlockNumber>>| {
            match last_send {
                Some(Some(block)) if block_number < block + T::GracePeriod::get() => {
                    Err(RECENTLY_SENT)
                }
                // In every other case we attempt to acquire the lock and send a transaction.
                _ => Ok(block_number),
            }
        });

        match res {
            // The value has been set correctly, which means we can safely send a transaction now.
            Ok(Ok(block_number)) => {
                let transaction_type = block_number % 3u32.into();
                if transaction_type == Zero::zero() {
                    TransactionType::Signed
                } else if transaction_type == T::BlockNumber::from(1u32) {
                    TransactionType::UnsignedForAny
                } else if transaction_type == T::BlockNumber::from(2u32) {
                    TransactionType::UnsignedForAll
                } else {
                    TransactionType::Raw
                }
            }
            // We are in the grace period, we should not send a transaction this time.
            Err(RECENTLY_SENT) => TransactionType::None,

            Ok(Err(_)) => TransactionType::None,
        }
    }

    /// A helper function to fetch the price and send signed transaction.
    fn fetch_price_and_send_signed() -> Result<(), &'static str> {
        let signer = Signer::<T, T::AuthorityId>::all_accounts();
        if !signer.can_sign() {
            return Err(
                "No local accounts available. Consider adding one via `author_insertKey` RPC.",
            )?;
        }
        let price = Self::fetch_price().map_err(|_| "Failed to fetch price")?;

        let results = signer.send_signed_transaction(|_account| Call::submit_price(price));

        for (acc, res) in &results {
            match res {
                Ok(()) => debug::info!("[{:?}] Submitted price of {} cents", acc.id, price),
                Err(e) => debug::error!("[{:?}] Failed to submit transaction: {:?}", acc.id, e),
            }
        }

        Ok(())
    }

    /// A helper function to fetch the price and send a raw unsigned transaction.
    fn fetch_price_and_send_raw_unsigned(block_number: T::BlockNumber) -> Result<(), &'static str> {
        // Make sure we don't fetch the price if unsigned transaction is going to be rejected
        // anyway.
        let next_unsigned_at = <NextUnsignedAt<T>>::get();
        if next_unsigned_at > block_number {
            return Err("Too early to send unsigned transaction");
        }

        let price = Self::fetch_price().map_err(|_| "Failed to fetch price")?;

        let call = Call::submit_price_unsigned(block_number, price);

        SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into())
            .map_err(|()| "Unable to submit unsigned transaction.")?;

        Ok(())
    }

    /// A helper function to fetch the price, sign payload and send an unsigned transaction
    fn fetch_price_and_send_unsigned_for_any_account(
        block_number: T::BlockNumber,
    ) -> Result<(), &'static str> {
        // Make sure we don't fetch the price if unsigned transaction is going to be rejected
        // anyway.
        let next_unsigned_at = <NextUnsignedAt<T>>::get();
        if next_unsigned_at > block_number {
            return Err("Too early to send unsigned transaction");
        }

        // Make an external HTTP request to fetch the current price.
        // Note this call will block until response is received.
        let price = Self::fetch_price().map_err(|_| "Failed to fetch price")?;

        // -- Sign using any account
        let (_, result) = Signer::<T, T::AuthorityId>::any_account()
            .send_unsigned_transaction(
                |account| PricePayload {
                    price,
                    block_number,
                    public: account.public.clone(),
                },
                |payload, signature| {
                    Call::submit_price_unsigned_with_signed_payload(payload, signature)
                },
            )
            .ok_or("No local accounts accounts available.")?;
        result.map_err(|()| "Unable to submit transaction")?;

        Ok(())
    }

    /// A helper function to fetch the price, sign payload and send an unsigned transaction
    fn fetch_price_and_send_unsigned_for_all_accounts(
        block_number: T::BlockNumber,
    ) -> Result<(), &'static str> {
        // Make sure we don't fetch the price if unsigned transaction is going to be rejected
        // anyway.
        let next_unsigned_at = <NextUnsignedAt<T>>::get();
        if next_unsigned_at > block_number {
            return Err("Too early to send unsigned transaction");
        }

        let price = Self::fetch_price().map_err(|_| "Failed to fetch price")?;

        // -- Sign using all accounts
        let transaction_results = Signer::<T, T::AuthorityId>::all_accounts()
            .send_unsigned_transaction(
                |account| PricePayload {
                    price,
                    block_number,
                    public: account.public.clone(),
                },
                |payload, signature| {
                    Call::submit_price_unsigned_with_signed_payload(payload, signature)
                },
            );
        for (_account_id, result) in transaction_results.into_iter() {
            if result.is_err() {
                return Err("Unable to submit transaction");
            }
        }

        Ok(())
    }

    /// Parse the price from the given JSON string using `lite-json`.
    ///
    /// Returns `None` when parsing failed or `Some(price in cents)` when parsing is successful.
    fn parse_price(price_str: &str) -> Option<u32> {
        let val = lite_json::parse_json(price_str);
        let price = val.ok().and_then(|v| match v {
            JsonValue::Object(obj) => {
                let mut chars = "USD".chars();
                obj.into_iter()
                    .find(|(k, _)| k.iter().all(|k| Some(*k) == chars.next()))
                    .and_then(|v| match v.1 {
                        JsonValue::Number(number) => Some(number),
                        _ => None,
                    })
            }
            _ => None,
        })?;

        let exp = price.fraction_length.checked_sub(2).unwrap_or(0);
        Some(price.integer as u32 * 100 + (price.fraction / 10_u64.pow(exp)) as u32)
    }

    /// Add new price to the list.
    fn add_price(who: T::AccountId, price: u32) {
        debug::info!("Adding to the average: {}", price);
        Prices::mutate(|prices| {
            const MAX_LEN: usize = 64;

            if prices.len() < MAX_LEN {
                prices.push(price);
            } else {
                prices[price as usize % MAX_LEN] = price;
            }
        });

        let average = Self::average_price()
            .expect("The average is not empty, because it was just mutated; qed");
        debug::info!("Current average price is: {}", average);
        // here we are raising the NewPrice event
        Self::deposit_event(RawEvent::NewPrice(price, who));
    }

    /// Calculate current average price.
    fn average_price() -> Option<u32> {
        let prices = Prices::get();
        if prices.is_empty() {
            None
        } else {
            Some(prices.iter().fold(0_u32, |a, b| a.saturating_add(*b)) / prices.len() as u32)
        }
    }

    fn validate_transaction_parameters(
        block_number: &T::BlockNumber,
        new_price: &u32,
    ) -> TransactionValidity {
        // Now let's check if the transaction has any chance to succeed.
        let next_unsigned_at = <NextUnsignedAt<T>>::get();
        if &next_unsigned_at > block_number {
            return InvalidTransaction::Stale.into();
        }
        // Let's make sure to reject transactions from the future.
        let current_block = <system::Module<T>>::block_number();
        if &current_block < block_number {
            return InvalidTransaction::Future.into();
        }

        let avg_price = Self::average_price()
            .map(|price| {
                if &price > new_price {
                    price - new_price
                } else {
                    new_price - price
                }
            })
            .unwrap_or(0);

        ValidTransaction::with_tag_prefix("ExampleOffchainWorker")
            .priority(T::UnsignedPriority::get().saturating_add(avg_price as _))
            .and_provides(next_unsigned_at)
            .longevity(5)
            .propagate(true)
            .build()
    }
}

#[allow(deprecated)] // ValidateUnsigned
impl<T: Trait> frame_support::unsigned::ValidateUnsigned for Module<T> {
    type Call = Call<T>;

    fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
        // Firstly let's check that we call the right function.
        if let Call::submit_price_unsigned_with_signed_payload(ref payload, ref signature) = call {
            let signature_valid =
                SignedPayload::<T>::verify::<T::AuthorityId>(payload, signature.clone());
            if !signature_valid {
                return InvalidTransaction::BadProof.into();
            }
            Self::validate_transaction_parameters(&payload.block_number, &payload.price)
        } else if let Call::submit_price_unsigned(block_number, new_price) = call {
            Self::validate_transaction_parameters(block_number, new_price)
        } else {
            InvalidTransaction::Call.into()
        }
    }
}
