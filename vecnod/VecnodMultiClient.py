# encoding: utf-8
import asyncio

from vecnod.VecnodClient import VecnodClient
# pipenv run python -m grpc_tools.protoc -I./protos --python_out=. --grpc_python_out=. ./protos/rpc.proto ./protos/messages.proto ./protos/p2p.proto
from vecnod.VecnodThread import VecnodCommunicationError


class VecnodMultiClient(object):
    def __init__(self, hosts: list[str]):
        self.vecnods = [VecnodClient(*h.split(":")) for h in hosts]

    def __get_vecnod(self):
        for k in self.vecnods:
            if k.is_utxo_indexed:
                return k

    async def initialize_all(self):
        tasks = [asyncio.create_task(k.ping()) for k in self.vecnods]

        for t in tasks:
            await t

    async def __request(self, command, params=None, timeout=60, retry=3):
        vecnod = self.__get_vecnod()
        if vecnod is not None: 
            return await vecnod.request(command, params, timeout=timeout, retry=retry)

    async def request(self, command, params=None, timeout=60, retry=3):
        try:
            return await self.__request(command, params, timeout=timeout, retry=retry)
        except VecnodCommunicationError:
            await self.initialize_all()
            return await self.__request(command, params, timeout=timeout, retry=retry)

    async def notify(self, command, params, callback):
        vecnod = self.__get_vecnod()
        if vecnod is not None: 
            return self.notify(command, params, callback)