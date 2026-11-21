#!/usr/bin/env python3
"""
Holodeck Python Agent Integration Example
----------------------------------------
Demonstrates how an autonomous coding agent or test runner interacts with
Holodeck's local Stripe simulator and event bus using standard Python libraries.

Usage:
  1. Start Holodeck: `holodeck start`
  2. Run this script: `python3 agent_stripe_test.py`
"""

import json
import urllib.request
import urllib.error
import sys

BASE_URL = "http://127.0.0.1:8787"

def request(path, method="GET", data=None):
    url = f"{BASE_URL}{path}"
    headers = {"Content-Type": "application/json"}
    body = json.dumps(data).encode("utf-8") if data else None
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.URLError as e:
        print(f"❌ Connection failed to {url}: {e}")
        print("💡 Ensure Holodeck is running via `holodeck start`")
        sys.exit(1)

def main():
    print("🚀 Running Holodeck Agent Stripe Integration Test...\n")

    # 1. Health check
    health = request("/health")
    print(f"✅ Holodeck Health: {health['status']} (v{health['version']})")

    # 2. Create customer
    customer = request("/v1/customers", method="POST", data={
        "email": "agent@autonomous.org",
        "name": "Dev Agent 007"
    })
    print(f"💳 Created Mock Stripe Customer: {customer['id']}")

    # 3. Create checkout session
    session = request("/v1/checkout/sessions", method="POST", data={
        "customer": customer["id"],
        "amount_total": 4900,
        "currency": "usd"
    })
    print(f"🛒 Created Checkout Session: {session['id']} (Status: {session['status']})")

    # 4. Simulate user completion
    completed = request(f"/v1/checkout/sessions/{session['id']}/complete", method="POST")
    print(f"🎉 Completed Checkout Session: Status={completed['status']}, Payment={completed['payment_status']}")

    # 5. Inspect EventBus to verify webhook event captured
    events = request("/events?provider=stripe&limit=5")
    print(f"\n📡 Verified Captured Events on Holodeck EventBus: {len(events)} events found")
    for evt in events:
        print(f"   - [{evt['provider'].upper()}] {evt['event_type']} (ID: {evt['id']})")

    print("\n✨ All Holodeck mock cloud tests succeeded!")

if __name__ == "__main__":
    main()
