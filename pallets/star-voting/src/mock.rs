use crate as pallet_star_voting;
use frame_support::derive_impl;
use frame_system::EnsureRoot;
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

#[frame_support::runtime]
mod runtime {
	// The main runtime
	#[runtime::runtime]
	// Runtime Types to be generated
	#[runtime::derive(
		RuntimeCall,
		RuntimeEvent,
		RuntimeError,
		RuntimeOrigin,
		RuntimeFreezeReason,
		RuntimeHoldReason,
		RuntimeSlashReason,
		RuntimeLockId,
		RuntimeTask,
		RuntimeViewFunction
	)]
	pub struct Test;

	#[runtime::pallet_index(0)]
	pub type System = frame_system::Pallet<Test>;

	#[runtime::pallet_index(1)]
	pub type StarVoting = pallet_star_voting::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
}

impl pallet_star_voting::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();
	type TallyOrigin = EnsureRoot<Self::AccountId>;
	type MaxCandidates = frame_support::traits::ConstU32<1000>;
	type MaxWinners = frame_support::traits::ConstU32<500>;
}
// Build genesis storage according to the mock runtime.
pub fn new_test_ext() -> sp_io::TestExternalities {
	frame_system::GenesisConfig::<Test>::default().build_storage().unwrap().into()
}
