-- #1670: プロフィールの NIP-05 の識別子(ADR 0064)。欄の無い profile は NULL。
ALTER TABLE profiles
    ADD COLUMN nip05 TEXT;
