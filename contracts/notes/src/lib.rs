#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    Address, Env, Map, String, Symbol, Vec,
};
 
// ─── Storage Keys ────────────────────────────────────────────────────────────
const ADMIN: Symbol = symbol_short!("ADMIN");
const ORDERS: Symbol = symbol_short!("ORDERS");
const SUPPLIERS: Symbol = symbol_short!("SUPPLIERS");
 
// ─── Data Types ──────────────────────────────────────────────────────────────
 
/// Lifecycle state of a purchase order
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum OrderStatus {
    Created,       // NGO created the order
    Accepted,      // Supplier confirmed they can fulfil
    Delivered,     // Supplier marked delivery complete
    Verified,      // NGO field officer verified receipt
    Paid,          // USDC released to supplier
    Disputed,      // Flagged for manual resolution
    Cancelled,     // Cancelled before delivery
}
 
/// A purchase order representing one supply chain payment event
#[contracttype]
#[derive(Clone)]
pub struct PurchaseOrder {
    pub id: u64,
    pub ngo: Address,           // NGO wallet that created the order
    pub supplier: Address,      // Supplier wallet that will be paid
    pub description: String,    // e.g. "500 kg rice – Kampala warehouse"
    pub usdc_amount: i128,      // Amount in stroops (1 USDC = 10_000_000)
    pub status: OrderStatus,
    pub created_at: u64,        // Ledger timestamp of creation
    pub delivery_proof: String, // IPFS CID or GPS coordinates from supplier
    pub verifier: Address,      // Field officer address authorized to verify
}
 
/// Minimal supplier record stored for trust and compliance checks
#[contracttype]
#[derive(Clone)]
pub struct Supplier {
    pub wallet: Address,
    pub name: String,
    pub is_approved: bool,      // NGO admin must whitelist before first payment
    pub total_paid: i128,       // Running total for audit trail
}
 
// ─── Contract ────────────────────────────────────────────────────────────────
#[contract]
pub struct AidFlow;
 
#[contractimpl]
impl AidFlow {
 
    /// Initialize the contract. Sets the NGO admin who controls supplier approval.
    /// Must be called once immediately after deployment.
    pub fn init(env: Env, admin: Address) {
        admin.require_auth();
        env.storage().instance().set(&ADMIN, &admin);
        // Initialize empty maps for orders and suppliers
        let orders: Map<u64, PurchaseOrder> = Map::new(&env);
        let suppliers: Map<Address, Supplier> = Map::new(&env);
        env.storage().instance().set(&ORDERS, &orders);
        env.storage().instance().set(&SUPPLIERS, &suppliers);
    }
 
    /// NGO admin approves a supplier wallet. Only approved suppliers can receive payments.
    /// This is a compliance checkpoint – equivalent to KYC/vendor vetting.
    pub fn approve_supplier(
        env: Env,
        name: String,
        supplier_wallet: Address,
    ) {
        let admin: Address = env.storage().instance().get(&ADMIN).unwrap();
        admin.require_auth(); // Only the NGO admin can whitelist suppliers
 
        let mut suppliers: Map<Address, Supplier> =
            env.storage().instance().get(&SUPPLIERS).unwrap();
 
        let supplier = Supplier {
            wallet: supplier_wallet.clone(),
            name,
            is_approved: true,
            total_paid: 0,
        };
        suppliers.set(supplier_wallet, supplier);
        env.storage().instance().set(&SUPPLIERS, &suppliers);
    }
 
    /// Create a new purchase order. Locks the intent to pay a supplier.
    /// The NGO specifies the supplier, amount, field verifier, and item description.
    /// Note: actual USDC escrow is held off-chain or via SEP-24 anchor; this contract
    /// manages the workflow state and releases trigger.
    pub fn create_order(
        env: Env,
        ngo: Address,
        supplier: Address,
        description: String,
        usdc_amount: i128,
        verifier: Address,
    ) -> u64 {
        ngo.require_auth(); // NGO must sign the order creation
 
        // Ensure supplier is approved before committing funds
        let suppliers: Map<Address, Supplier> =
            env.storage().instance().get(&SUPPLIERS).unwrap();
        let supplier_record = suppliers.get(supplier.clone())
            .expect("Supplier not approved. Admin must approve supplier first.");
        assert!(supplier_record.is_approved, "Supplier account is not active");
        assert!(usdc_amount > 0, "Amount must be positive");
 
        let mut orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
 
        // Use order count as auto-increment ID (deterministic, no randomness needed)
        let order_id = orders.len() as u64;
 
        let order = PurchaseOrder {
            id: order_id,
            ngo,
            supplier,
            description,
            usdc_amount,
            status: OrderStatus::Created,
            created_at: env.ledger().timestamp(),
            delivery_proof: String::from_str(&env, ""),
            verifier,
        };
 
        orders.set(order_id, order);
        env.storage().instance().set(&ORDERS, &orders);
 
        order_id // Return the new order ID so the frontend can track it
    }
 
    /// Supplier accepts the order, signalling they can fulfil it.
    /// Prevents the NGO from creating a phantom commitment the supplier never saw.
    pub fn accept_order(env: Env, supplier: Address, order_id: u64) {
        supplier.require_auth();
 
        let mut orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
        let mut order = orders.get(order_id).expect("Order not found");
 
        // Only the designated supplier for this order may accept
        assert!(order.supplier == supplier, "Not the designated supplier");
        assert!(order.status == OrderStatus::Created, "Order not in Created state");
 
        order.status = OrderStatus::Accepted;
        orders.set(order_id, order);
        env.storage().instance().set(&ORDERS, &orders);
    }
 
    /// Supplier submits delivery proof (IPFS CID or GPS string) after drop-off.
    /// This is the on-chain receipt that triggers the verification step.
    pub fn submit_delivery(
        env: Env,
        supplier: Address,
        order_id: u64,
        proof: String, // IPFS CID of photo/doc or GPS coordinates
    ) {
        supplier.require_auth();
 
        let mut orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
        let mut order = orders.get(order_id).expect("Order not found");
 
        assert!(order.supplier == supplier, "Not the designated supplier");
        assert!(order.status == OrderStatus::Accepted, "Order not accepted yet");
 
        order.delivery_proof = proof;
        order.status = OrderStatus::Delivered;
        orders.set(order_id, order);
        env.storage().instance().set(&ORDERS, &orders);
    }
 
    /// Field officer (verifier) confirms physical receipt of goods.
    /// This is the critical gate before USDC is released to the supplier.
    /// In practice the frontend calls the anchor/payment rail after this succeeds.
    pub fn verify_delivery(env: Env, verifier: Address, order_id: u64) {
        verifier.require_auth();
 
        let mut orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
        let mut order = orders.get(order_id).expect("Order not found");
 
        assert!(order.verifier == verifier, "Not the authorized verifier");
        assert!(order.status == OrderStatus::Delivered, "Delivery not submitted yet");
 
        order.status = OrderStatus::Verified;
        orders.set(order_id, order);
        env.storage().instance().set(&ORDERS, &orders);
 
        // Emit event so off-chain listener triggers USDC payment via Stellar anchor
        env.events().publish(
            (symbol_short!("verified"), order_id),
            order.usdc_amount,
        );
    }
 
    /// Mark order as paid. Called by the NGO after the USDC transfer is confirmed.
    /// Updates the supplier's running total for audit purposes.
    pub fn mark_paid(env: Env, ngo: Address, order_id: u64) {
        ngo.require_auth();
 
        let mut orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
        let mut order = orders.get(order_id).expect("Order not found");
 
        assert!(order.ngo == ngo, "Not the originating NGO");
        assert!(order.status == OrderStatus::Verified, "Delivery not verified");
 
        order.status = OrderStatus::Paid;
        orders.set(order_id, order.clone());
        env.storage().instance().set(&ORDERS, &orders);
 
        // Update supplier cumulative total for donor audit trail
        let mut suppliers: Map<Address, Supplier> =
            env.storage().instance().get(&SUPPLIERS).unwrap();
        let mut supplier_record = suppliers.get(order.supplier.clone()).unwrap();
        supplier_record.total_paid += order.usdc_amount;
        suppliers.set(order.supplier, supplier_record);
        env.storage().instance().set(&SUPPLIERS, &suppliers);
    }
 
    /// Flag an order for dispute. Either party can raise a dispute.
    /// Freezes the order state until admin resolution.
    pub fn raise_dispute(env: Env, caller: Address, order_id: u64) {
        caller.require_auth();
 
        let mut orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
        let mut order = orders.get(order_id).expect("Order not found");
 
        // Only the NGO, supplier, or verifier on this order may dispute
        let is_party = order.ngo == caller
            || order.supplier == caller
            || order.verifier == caller;
        assert!(is_party, "Caller is not a party to this order");
        assert!(
            order.status != OrderStatus::Paid && order.status != OrderStatus::Cancelled,
            "Cannot dispute a completed order"
        );
 
        order.status = OrderStatus::Disputed;
        orders.set(order_id, order);
        env.storage().instance().set(&ORDERS, &orders);
    }
 
    /// Read a single order by ID. Used by the frontend dashboard.
    pub fn get_order(env: Env, order_id: u64) -> PurchaseOrder {
        let orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
        orders.get(order_id).expect("Order not found")
    }
 
    /// Read all order IDs. Frontend paginates and fetches individually.
    pub fn get_order_ids(env: Env) -> Vec<u64> {
        let orders: Map<u64, PurchaseOrder> =
            env.storage().instance().get(&ORDERS).unwrap();
        orders.keys()
    }
 
    /// Get supplier details including cumulative payments (for donor reports).
    pub fn get_supplier(env: Env, wallet: Address) -> Supplier {
        let suppliers: Map<Address, Supplier> =
            env.storage().instance().get(&SUPPLIERS).unwrap();
        suppliers.get(wallet).expect("Supplier not found")
    }
}
