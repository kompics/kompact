#!/usr/bin/env python
import socket
import struct
import sys
import time

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
server_addr = ('127.0.0.1', 8080)
sock.connect(server_addr)
MAGIC=0xC0A1BA11
seq_num  = 0
HEADER_LEN = 9
INSTRUCTIONS = """
TYPE    FRAMETYPE
1       stream_request  stream_id   capacity
2       data            stream_id   data...
3       credit_update   stream_id   capacity
99      ping
"""

def send_stream_request(pieces):
    if len(pieces) != 3:
        print("stream_request takes 3 args")
        return
    frame_type = int(pieces[0])
    stream_id = int(pieces[1])
    capacity = int(pieces[2])
    length = HEADER_LEN + 8

    length = struct.pack('>I', length)
    magic = struct.pack('>I', MAGIC)
    frame_type = struct.pack('B', frame_type)
    stream_id = struct.pack('>I', stream_id)
    capacity = struct.pack('>I', capacity)

    b = bytearray()
    b.extend(length)
    b.extend(magic)
    b.extend(frame_type)
    b.extend(stream_id)
    b.extend(capacity)
    sock.sendall(b)

def send_credit_update(pieces):
    if len(pieces) != 3:
        print("credit_update takes 3 args")
        return
    frame_type = int(pieces[0])
    stream_id = int(pieces[1])
    capacity = int(pieces[2])
    length = HEADER_LEN + 8

    length = struct.pack('>I', length)
    magic = struct.pack('>I', MAGIC)
    frame_type = struct.pack('B', frame_type)
    stream_id = struct.pack('>I', stream_id)
    capacity = struct.pack('>I', capacity)

    b = bytearray()
    b.extend(length)
    b.extend(magic)
    b.extend(frame_type)
    b.extend(stream_id)
    b.extend(capacity)
    sock.sendall(b)

def send_ping():
    frame_type = 99
    length = HEADER_LEN

    length = struct.pack('>I', length)
    magic = struct.pack('>I', MAGIC)
    frame_type = struct.pack('B', frame_type)

    b = bytearray()
    b.extend(length)
    b.extend(magic)
    b.extend(frame_type)
    sock.sendall(b)



def send_data(pieces):
    if len(pieces) < 3:
        print("data takes 3+ args")
        return
    frame_type = int(pieces[0])
    stream_id = int(pieces[1])
    data = ''.join(pieces[2:])

    length = HEADER_LEN + 12 + len(data)

    encoded_length = struct.pack('>I', length)
    encoded_magic = struct.pack('>I', MAGIC)

    encoded_stream_id = struct.pack('>I', stream_id)

    encoded_frame_type = struct.pack('B', frame_type)
    encoded_seq_num = struct.pack('>I', seq_num)
    encoded_payload_len = struct.pack('>I', len(data))

    b = bytearray()
    b.extend(encoded_length)
    b.extend(encoded_magic)
    b.extend(encoded_frame_type)
    b.extend(encoded_stream_id)
    b.extend(encoded_seq_num)
    b.extend(encoded_payload_len)
    b.extend(data)

    sock.sendall(b)


try:
    while True:
        print(INSTRUCTIONS)
        data = raw_input('>> ')
        if data == "q" or data == "quit":
            exit(0)
        pieces = data.split(" ")

        frame_type = int(pieces[0])

        if frame_type == 1:     # StreamRequest
            send_stream_request(pieces)
        elif frame_type == 2:   # Data
            send_data(pieces)
            # Try to receive anything (just in case!)
            try:
                res = sock.recv(2048)
                print(res)
            except:
                pass
        elif frame_type == 3:   # CreditUpdate
            send_credit_update(pieces)
        elif frame_type == 99:  # Ping
            send_ping()
        seq_num += 1

finally:
    print("Closing socket")
    sock.close()
