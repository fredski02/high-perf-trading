#!/usr/bin/env python3
"""
Test Cancel and Replace reservation management
"""
import socket
import json
import struct
import time

def send_json_command(sock, cmd_dict):
    """Send a JSON command with length-delimited framing"""
    json_str = json.dumps(cmd_dict)
    json_bytes = json_str.encode('utf-8')
    length = len(json_bytes)
    # Little-endian 32-bit length prefix
    frame = struct.pack('<I', length) + json_bytes
    sock.sendall(frame)
    print(f"  → Sent: {json_str[:80]}...")

def recv_json_response(sock, timeout=2.0):
    """Receive a JSON response with length-delimited framing"""
    sock.settimeout(timeout)
    try:
        # Read 4-byte length prefix
        length_bytes = sock.recv(4)
        if not length_bytes:
            return None
        length = struct.unpack('<I', length_bytes)[0]
        
        # Read the JSON payload
        json_bytes = b''
        while len(json_bytes) < length:
            chunk = sock.recv(length - len(json_bytes))
            if not chunk:
                break
            json_bytes += chunk
        
        response = json.loads(json_bytes.decode('utf-8'))
        print(f"  ← Received: {json.dumps(response)[:100]}...")
        return response
    except socket.timeout:
        print("  ← (timeout)")
        return None
    except Exception as e:
        print(f"  ← Error: {e}")
        return None

def main():
    print("=" * 60)
    print("Cancel & Replace Reservation Test")
    print("=" * 60)
    print()
    
    # Connect to gateway JSON port
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.connect(('127.0.0.1', 9001))
    print("✓ Connected to gateway (JSON port 9001)")
    print()
    
    try:
        # Test 1: Cancel releases reservation
        print("Test 1: Cancel releases reservation")
        print("-" * 60)
        
        # Authenticate
        print("1. Authenticate as account 7...")
        send_json_command(sock, {"Authenticate": {"api_key": "test-key-7"}})
        recv_json_response(sock)
        time.sleep(0.1)
        
        # Query initial state
        print("\n2. Query initial account state...")
        send_json_command(sock, {
            "QueryAccount": {
                "client_seq": 1,
                "account_id": 7,
                "symbol_id": 1
            }
        })
        initial_state = recv_json_response(sock)
        time.sleep(0.1)
        
        # Place order (reserves buying power)
        print("\n3. Place buy order (50k @ 1 qty = 50k reserved)...")
        send_json_command(sock, {
            "NewOrder": {
                "client_seq": 2,
                "order_id": 7001,
                "account_id": 7,
                "symbol_id": 1,
                "side": "Buy",
                "price": 50000,
                "qty": 1,
                "tif": "Gtc",
                "flags": {"post_only": True}
            }
        })
        # Receive Ack
        recv_json_response(sock)
        time.sleep(0.1)
        
        # Query state after order (should show tentative_reserved)
        print("\n4. Query account state (should have 50k reserved)...")
        send_json_command(sock, {
            "QueryAccount": {
                "client_seq": 3,
                "account_id": 7,
                "symbol_id": 1
            }
        })
        after_order = recv_json_response(sock)
        time.sleep(0.1)
        
        # Cancel the order (should release reservation)
        print("\n5. Cancel order 7001 (should release 50k)...")
        send_json_command(sock, {
            "Cancel": {
                "client_seq": 4,
                "account_id": 7,
                "symbol_id": 1,
                "order_id": 7001
            }
        })
        # Receive CancelAck
        recv_json_response(sock)
        time.sleep(0.1)
        
        # Query state after cancel (reservation should be released)
        print("\n6. Query account state (reservation should be released)...")
        send_json_command(sock, {
            "QueryAccount": {
                "client_seq": 5,
                "account_id": 7,
                "symbol_id": 1
            }
        })
        after_cancel = recv_json_response(sock)
        time.sleep(0.1)
        
        # Place another order with same funds (should succeed)
        print("\n7. Place another 50k order (should succeed - funds released)...")
        send_json_command(sock, {
            "NewOrder": {
                "client_seq": 6,
                "order_id": 7002,
                "account_id": 7,
                "symbol_id": 1,
                "side": "Buy",
                "price": 50000,
                "qty": 1,
                "tif": "Gtc",
                "flags": {"post_only": True}
            }
        })
        recv_json_response(sock)
        time.sleep(0.1)
        
        print("\n✓ Test 1 complete")
        
        # Test 2: Replace adjusts reservation
        print("\n\nTest 2: Replace adjusts reservation")
        print("-" * 60)
        
        # Place order
        print("\n1. Place buy order (60k @ 1 qty = 60k reserved)...")
        send_json_command(sock, {
            "NewOrder": {
                "client_seq": 7,
                "order_id": 7003,
                "account_id": 7,
                "symbol_id": 1,
                "side": "Buy",
                "price": 60000,
                "qty": 1,
                "tif": "Gtc",
                "flags": {"post_only": True}
            }
        })
        recv_json_response(sock)
        time.sleep(0.1)
        
        # Replace with higher price (adjust reservation up)
        print("\n2. Replace with higher price (70k - adjust up)...")
        send_json_command(sock, {
            "Replace": {
                "client_seq": 8,
                "account_id": 7,
                "symbol_id": 1,
                "order_id": 7003,
                "new_price": 70000,
                "new_qty": 1
            }
        })
        recv_json_response(sock)
        time.sleep(0.1)
        
        # Replace with lower price (adjust reservation down)
        print("\n3. Replace with lower price (40k - adjust down)...")
        send_json_command(sock, {
            "Replace": {
                "client_seq": 9,
                "account_id": 7,
                "symbol_id": 1,
                "order_id": 7003,
                "new_price": 40000,
                "new_qty": 1
            }
        })
        recv_json_response(sock)
        time.sleep(0.1)
        
        # Cancel to clean up
        print("\n4. Cancel order to clean up...")
        send_json_command(sock, {
            "Cancel": {
                "client_seq": 10,
                "account_id": 7,
                "symbol_id": 1,
                "order_id": 7003
            }
        })
        recv_json_response(sock)
        
        print("\n✓ Test 2 complete")
        
    finally:
        sock.close()
    
    print("\n" + "=" * 60)
    print("All tests complete!")
    print("=" * 60)
    print("\nCheck gateway logs for detailed reservation tracking:")
    print("  grep 'reservation\\|tentative_reserved' gateway.log")

if __name__ == '__main__':
    main()
