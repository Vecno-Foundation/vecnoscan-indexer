from sqlalchemy import Column, String, BigInteger, UniqueConstraint, Index, Sequence

from dbsession import Base

class Balance(Base):
    __tablename__ = 'balances'

    script_public_key_address = Column(String, primary_key=True)
    balance = Column(BigInteger, default=0)

    __table_args__ = (UniqueConstraint('script_public_key_address', name='balances_address_key'),)

    def __init__(self, script_public_key_address, balance=0):
        self.script_public_key_address = script_public_key_address
        self.balance = balance

Index("idx_address", Balance.script_public_key_address)
Index("idx_balance", Balance.balance)
