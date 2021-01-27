module.exports = {
    docs: {
	"About": [
	    "introduction",
	    "terminology",
	    "history",
	],
	"Wallets": [
	    "wallet-guide",
	    "wallet-guide/apps",
	    {
		type: "category",
		label: "Web Wallets",
		items: [
		    "wallet-guide/web-wallets",
		    "wallet-guide/solflare",
		],
	    },
	    {
		type: "category",
		label: "Hardware Wallets",
		items: [
		    "wallet-guide/ledger-live",
		],
	    },
	    {
		type: "category",
		label: "Command-line Wallets",
		items: [
		    "wallet-guide/cli",
		    "wallet-guide/paper-wallet",
		    {
			type: "category",
			label: "Hardware Wallets",
			items: [
			    "wallet-guide/hardware-wallets",
			    "wallet-guide/hardware-wallets/ledger",
			],
		    },
		    "wallet-guide/file-system-wallet",
		],
	    },
	    "wallet-guide/support",
	],
	"Staking": [
	    "staking",
	    "staking/stake-accounts",
	],
	"Command Line": [
	    "cli",
	    "cli/install-solana-cli-tools",
	    "cli/conventions",
	    "cli/choose-a-cluster",
	    "cli/transfer-tokens",
	    "cli/delegate-stake",
	    "cli/manage-stake-accounts",
	    "offline-signing",
	    "offline-signing/durable-nonce",
	    "cli/usage",
	],
	"Developing": [
	    {
		type: "category",
		label: "Programming Model",
		items: [
		    "developing/programming-model/overview",
		    "developing/programming-model/transactions",
		    "developing/programming-model/accounts",
		    "developing/programming-model/runtime",
		    "developing/programming-model/calling-between-programs",
		],
	    },
	    {
		type: "category",
		label: "Clients",
		items: [
		    "developing/clients/jsonrpc-api",
		    "developing/clients/javascript-api",
		],
	    },
	    {
		type: "category",
		label: "Builtins",
		items: [
		    "developing/builtins/programs",
		    "developing/builtins/sysvars",
		],
	    },
	    {
		type: "category",
		label: "Deployed Programs",
		items: [
		    "developing/deployed-programs/overview",
		    "developing/deployed-programs/developing-rust",
		    "developing/deployed-programs/developing-c",
		    "developing/deployed-programs/deploying",
		    "developing/deployed-programs/debugging",
		    "developing/deployed-programs/examples",
		    "developing/deployed-programs/faq",
		],
	    },
	    "developing/backwards-compatibility",
	],
	"Integrating": ["integrations/exchange"],
	"Validating": [
	    "running-validator",
	    "running-validator/validator-reqs",
	    "running-validator/validator-start",
	    "running-validator/vote-accounts",
	    "running-validator/validator-stake",
	    "running-validator/validator-monitor",
	    "running-validator/validator-info",
	    {
		type: "category",
		label: "Incenvitized Testnet",
		items: [
		    "tour-de-sol",
		    {
			type: "category",
			label: "Registration",
			items: [
			    "tour-de-sol/registration/how-to-register",
			    "tour-de-sol/registration/terms-of-participation",
			    "tour-de-sol/registration/rewards",
			    "tour-de-sol/registration/confidentiality",
			    "tour-de-sol/registration/validator-registration-and-rewards-faq",
			],
		    },
		    {
			type: "category",
			label: "Participation",
			items: [
			    "tour-de-sol/participation/validator-technical-requirements",
			    "tour-de-sol/participation/validator-public-key-registration",
			    "tour-de-sol/participation/steps-to-create-a-validator",
			],
		    },
		    "tour-de-sol/useful-links",
		    "tour-de-sol/submitting-bugs",
		],
	    },
	    "running-validator/validator-troubleshoot",
	],
	"Clusters": [
	    "clusters",
	    "cluster/rpc-endpoints",
	    "cluster/bench-tps",
	    "cluster/performance-metrics"
	],
	"Architecture": [
	    {
		type: "category",
		label: "Cluster",
		items: [
		    "cluster/overview",
		    "cluster/synchronization",
		    "cluster/leader-rotation",
		    "cluster/fork-generation",
		    "cluster/managing-forks",
		    "cluster/turbine-block-propagation",
		    "cluster/vote-signing",
		    "cluster/stake-delegation-and-rewards",
		],
	    },
	    {
		type: "category",
		label: "Validator",
		items: [
		    "validator/anatomy",
		    "validator/tpu",
		    "validator/tvu",
		    "validator/blockstore",
		    "validator/gossip",
		    "validator/runtime",
		],
	    },
	],
	"Economics": [
	    "economics_overview",
	    {
                type: "category",
                label: "Inflation Design",
                items: [
                    "inflation/terminology",
		    "inflation/inflation_schedule",
                    "inflation/adjusted_staking_yield",
                ],
            },
	    "transaction_fees",
	    "storage_rent_economics"
	],
	"Design Proposals": [
	    {
		type: "category",
		label: "Implemented",
		items: [
		    "implemented-proposals/implemented-proposals",
		    "implemented-proposals/abi-management",
		    "implemented-proposals/bank-timestamp-correction",
		    "implemented-proposals/commitment",
		    "implemented-proposals/durable-tx-nonces",
		    "implemented-proposals/installer",
		    "implemented-proposals/instruction_introspection",
		    "implemented-proposals/leader-leader-transition",
		    "implemented-proposals/leader-validator-transition",
		    "implemented-proposals/persistent-account-storage",
		    "implemented-proposals/readonly-accounts",
		    "implemented-proposals/reliable-vote-transmission",
		    "implemented-proposals/rent",
		    "implemented-proposals/repair-service",
		    "implemented-proposals/rpc-transaction-history",
		    "implemented-proposals/snapshot-verification",
		    "implemented-proposals/staking-rewards",
		    "implemented-proposals/testing-programs",
		    "implemented-proposals/tower-bft",
		    "implemented-proposals/transaction-fees",
		    "implemented-proposals/validator-timestamp-oracle",
		],
	    },
	    {
		type: "category",
		label: "Accepted",
		items: [
		    "proposals/accepted-design-proposals",
		    "proposals/ledger-replication-to-implement",
		    "proposals/optimistic-confirmation-and-slashing",
		    "proposals/vote-signing-to-implement",
		    "proposals/cluster-test-framework",
		    "proposals/validator-proposal",
		    "proposals/simple-payment-and-state-verification",
		    "proposals/interchain-transaction-verification",
		    "proposals/snapshot-verification",
		    "proposals/bankless-leader",
		    "proposals/slashing",
		    "proposals/tick-verification",
		    "proposals/block-confirmation",
		    "proposals/rust-clients",
		    "proposals/optimistic_confirmation",
		    "proposals/embedding-move",
		    "proposals/rip-curl",
		]
	    },
	],
    },
};
