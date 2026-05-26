#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env, String};

    fn setup() -> (Env, AidFlowClient<'static>, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AidFlow);
        let client = AidFlowClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let supplier_wallet = Address::generate(&env);
        let verifier = Address::generate(&env);
        let ngo = Address::generate(&env);

        client.init(&admin);
        client.approve_supplier(
            &String::from_str(&env, "Kampala Grain Co."),
            &supplier_wallet,
        );

        (env, client, admin, ngo, supplier_wallet, verifier)
    }

    // Test 1: Happy path — full MVP flow from order creation to payment
    #[test]
    fn test_happy_path_full_order_lifecycle() {
        let (env, client, _admin, ngo, supplier, verifier) = setup();

        // NGO creates purchase order for 500 USDC worth of rice
        let order_id = client.create_order(
            &ngo,
            &supplier,
            &String::from_str(&env, "500 kg rice – Kampala warehouse"),
            &5_000_000_000i128, // 500 USDC in stroops
            &verifier,
        );
        assert_eq!(order_id, 0);

        // Supplier accepts
        client.accept_order(&supplier, &order_id);

        // Supplier submits IPFS delivery proof
        client.submit_delivery(
            &supplier,
            &order_id,
            &String::from_str(&env, "QmXvZ9aBcDeFgHiJkLmNoPqRsTuVwXyZ1234567890"),
        );

        // Field officer verifies receipt
        client.verify_delivery(&verifier, &order_id);

        // NGO marks paid after USDC transfer
        client.mark_paid(&ngo, &order_id);

        // Assert final state
        let order = client.get_order(&order_id);
        assert_eq!(order.status, OrderStatus::Paid);

        // Supplier's cumulative total updated
        let supplier_record = client.get_supplier(&supplier);
        assert_eq!(supplier_record.total_paid, 5_000_000_000i128);
    }

    // Test 2: Edge case — unapproved supplier cannot receive a purchase order
    #[test]
    #[should_panic(expected = "Supplier not approved")]
    fn test_unapproved_supplier_blocked() {
        let (env, client, _admin, ngo, _supplier, verifier) = setup();
        let rogue_supplier = Address::generate(&env);

        // Attempt to create an order with a supplier not on the whitelist
        client.create_order(
            &ngo,
            &rogue_supplier,
            &String::from_str(&env, "Fake goods"),
            &1_000_000_000i128,
            &verifier,
        );
    }

    // Test 3: State verification — order storage reflects correct status after each step
    #[test]
    fn test_state_transitions_stored_correctly() {
        let (env, client, _admin, ngo, supplier, verifier) = setup();

        let order_id = client.create_order(
            &ngo,
            &supplier,
            &String::from_str(&env, "Medical kits – Nairobi depot"),
            &2_000_000_000i128,
            &verifier,
        );

        let order = client.get_order(&order_id);
        assert_eq!(order.status, OrderStatus::Created);

        client.accept_order(&supplier, &order_id);
        assert_eq!(client.get_order(&order_id).status, OrderStatus::Accepted);

        let proof = String::from_str(&env, "QmTestProofHash");
        client.submit_delivery(&supplier, &order_id, &proof);
        let order_after_delivery = client.get_order(&order_id);
        assert_eq!(order_after_delivery.status, OrderStatus::Delivered);
        // Proof is persisted
        assert_eq!(order_after_delivery.delivery_proof, proof);

        client.verify_delivery(&verifier, &order_id);
        assert_eq!(client.get_order(&order_id).status, OrderStatus::Verified);
    }

    // Test 4: Dispute raised by the supplier before verification
    #[test]
    fn test_supplier_can_raise_dispute() {
        let (env, client, _admin, ngo, supplier, verifier) = setup();

        let order_id = client.create_order(
            &ngo,
            &supplier,
            &String::from_str(&env, "Blankets – Port-au-Prince"),
            &500_000_000i128,
            &verifier,
        );
        client.accept_order(&supplier, &order_id);
        client.submit_delivery(
            &supplier,
            &order_id,
            &String::from_str(&env, "QmDisputeProof"),
        );

        // Supplier disputes (e.g. wrong quantity received acknowledgement)
        client.raise_dispute(&supplier, &order_id);
        assert_eq!(client.get_order(&order_id).status, OrderStatus::Disputed);
    }

    // Test 5: Wrong verifier cannot approve delivery
    #[test]
    #[should_panic(expected = "Not the authorized verifier")]
    fn test_unauthorized_verifier_rejected() {
        let (env, client, _admin, ngo, supplier, verifier) = setup();
        let impostor = Address::generate(&env);

        let order_id = client.create_order(
            &ngo,
            &supplier,
            &String::from_str(&env, "Tents – Mogadishu"),
            &1_000_000_000i128,
            &verifier,
        );
        client.accept_order(&supplier, &order_id);
        client.submit_delivery(
            &supplier,
            &order_id,
            &String::from_str(&env, "QmImpostorProof"),
        );

        // Impostor tries to verify — must panic
        client.verify_delivery(&impostor, &order_id);
    }
}
