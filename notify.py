import base64
import json
import logging
import asyncio

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives import padding
from cryptography.hazmat.backends import default_backend

from amqtt.client import MQTTClient
from amqtt.mqtt.constants import QOS_1, QOS_2

def encrypt_aes_cbc(key, iv, plaintext):
    cipher = Cipher(algorithms.AES(key), modes.CBC(iv), backend=default_backend())
    encryptor = cipher.encryptor()
    padder = padding.PKCS7(128).padder()
    padded_data = padder.update(plaintext.encode()) + padder.finalize()
    ciphertext = encryptor.update(padded_data) + encryptor.finalize()
    return ciphertext

def encrypt_dcn(data):
  key = "b14ca5898a4e4142aace2ea2143a2410".encode("utf-8")
  bs = 16
  iv = bytes([0]*16)
  encrypted = encrypt_aes_cbc(key, iv, data)
  return base64.b64encode(encrypted)

logger = logging.getLogger(__name__)

async def uptime_coro():
    client = MQTTClient()
    await client.connect('wss://mqttserver.nl/')

    event = json.dumps({
      "type":0,
      "area":0,
      "location": "¯\\_(ツ)_/¯",
      "time":0,
      "description": "Example description,
      "group":0,
      "start":"2023-05-24T16:30:00.3458551+01:00",
      "eventurl": None,
      "eventOrganizer":"¯\\_(ツ)_/¯"
    })

    await client.publish("/DCN.Client/events", encrypt_dcn(event))
    await client.disconnect()

if __name__ == '__main__':
    formatter = "[%(asctime)s] %(name)s {%(filename)s:%(lineno)d} %(levelname)s - %(message)s"
    logging.basicConfig(level=logging.DEBUG, format=formatter)
    asyncio.get_event_loop().run_until_complete(uptime_coro())

