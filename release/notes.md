## VIBE 0.2.7

### Kijelölési javítás

- Normál egérhúzás csak a natív CLI beviteli promptjában jelöl ki, és nem fut túl a prompt végén.
- Nincs kék kijelölés az üres terminálterületen vagy a Codex sessionválasztóján. A sessionválasztó Ctrl+A archiválása a Codex saját parancsa marad.
- Korábbi kimenet másolása: Shift + egérhúzás, majd Ctrl+C. Ez csak olvasható kijelölés; Backspace/Delete nem módosítja. Üres sorokat és jobb oldali margót nem fest be.

### Az előző nyilvános kiadás óta

- OpenRouter saját coding agent terminálszerű felülettel, közvetlen prompttal és modell/projekt állapotsorral.
- Workspace fájlböngésző, Git-változásnézet és fájlcsatolás. OpenRouter-fájlmódosítások előnézettel és felhasználói jóváhagyással.
- Globális és workspace-szintű szabályok, workspace-enként kikapcsolható VIBE-örökléssel és opcionális helyi szabályfájllal.
- Legfeljebb 10 lépéses beviteli visszavonás, kijelölt szöveg törlésének javításai.
- Friss indításkor CLI-választó és üres workspace-lista; nincs automatikus System32-projekt vagy mintapanel.
- A mentett projektek, beszélgetések és fiókbeállítások frissítéskor megmaradnak. Natív CLI-knél a módosított VIBE-szabályok a CLI újraindításakor lépnek érvénybe.
