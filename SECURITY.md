# Security Policy

## Supported versions

Only the [latest release](https://github.com/eatsbricks/InGame/releases/latest) receives fixes.

## Reporting a vulnerability

Please **do not** open a public issue for security problems.

Report it privately through **[GitHub Security Advisories](https://github.com/eatsbricks/InGame/security/advisories/new)** and include:

- the InGame and Windows versions
- steps to reproduce
- what an attacker could do with it

You will get a reply as soon as possible, usually within a week.

## Verifying downloads

Every release lists the SHA-256 checksum of each file. Only download InGame from this repository's [Releases](https://github.com/eatsbricks/InGame/releases) page. Copies from other sites may have been modified.

```powershell
Get-FileHash .\InGame-Setup-0.2.0.exe -Algorithm SHA256
```

---

## นโยบายความปลอดภัย (ภาษาไทย)

หากพบช่องโหว่ด้านความปลอดภัย **กรุณาอย่าเปิด Issue สาธารณะ** ให้แจ้งแบบส่วนตัวผ่าน [GitHub Security Advisories](https://github.com/eatsbricks/InGame/security/advisories/new) แทน

ดาวน์โหลด InGame จากหน้า [Releases](https://github.com/eatsbricks/InGame/releases) ของ repository นี้เท่านั้น และตรวจค่า SHA-256 ให้ตรงกับที่ระบุไว้ ไฟล์จากเว็บอื่นอาจถูกดัดแปลง
