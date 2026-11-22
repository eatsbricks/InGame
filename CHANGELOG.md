# Changelog

การเปลี่ยนแปลงทั้งหมดของ InGame จะถูกบันทึกไว้ที่นี่

รูปแบบอ้างอิงจาก [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) และใช้เลขเวอร์ชันแบบ [Semantic Versioning](https://semver.org/)

## [Unreleased]

## [0.2.0] - 2026-11-22

เวอร์ชันแรกที่เปิดให้ดาวน์โหลดสาธารณะ 🎉

### Added

- **Auto-EN บน Windows 10 และ 11** สลับเป็นแป้นพิมพ์ภาษาอังกฤษทันทีที่เข้าเกมผ่าน Text Services Framework รองรับทุกภาษาและ IME แล้วคืนภาษาเดิมเมื่อพับจอออก
- **กันปุ่ม Caps Lock** ปิด Caps Lock ที่ค้างอยู่ตอนเข้าเกม และบล็อกปุ่มระหว่างเล่น พร้อมสวิตช์เปิด/ปิดแยก
- ตัวติดตั้ง NSIS แบบไม่ต้องใช้สิทธิ์ Admin

### Changed

- เปลี่ยนชื่อโปรแกรมจาก PUBG Helper เป็น **InGame**
- เปลี่ยน License เป็น PolyForm Noncommercial 1.0.0

## 0.1.0 - 2026-11-20

เวอร์ชันทดลองใช้ภายใน ไม่ได้เผยแพร่สาธารณะ

### Added

- ล็อกภาษาอังกฤษ บล็อก `Win+Space` และปุ่ม `~` ที่ใช้สลับภาษา
- กันพับจอเกม บล็อก `Win`, `Alt+Tab`, `Alt+Esc`, `Ctrl+Esc` และปิดปุ่มลัด Sticky Keys, Toggle Keys, Filter Keys ชั่วคราว พร้อมกู้คืนค่าอัตโนมัติหากโปรแกรมถูกปิดกะทันหัน
- ปุ่มพับจอออก (`F10`) ที่เปลี่ยนปุ่มได้
- ไอคอน System Tray พร้อมสถานะ Active / Standby / Paused
- เปิดพร้อม Windows แบบซ่อนใน Tray
- หน้าต่างตั้งค่าแบบมินิมอลที่ปิดตัวเองเพื่อคืน RAM เมื่อซ่อน

[Unreleased]: https://github.com/eatsbricks/InGame/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/eatsbricks/InGame/releases/tag/v0.2.0
