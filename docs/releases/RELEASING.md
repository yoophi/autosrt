# AutoSRT 릴리스 정책

## CalVer

버전 형식은 `YYYY.M.PATCH`다. 연도는 4자리, 월은 앞자리 0 없이 1–12, PATCH는 0부터 시작하는 정수다. 기준 날짜는 Asia/Seoul의 릴리스 게시일이다.

- 2026년 10월 첫 릴리스: `2026.10.0`.
- 같은 달의 다음 릴리스: `2026.10.1`, `2026.10.2`.
- 다음 달 첫 릴리스: `2026.11.0`. PATCH는 월이 바뀌면 0으로 초기화한다.
- 태그는 `v2026.10.0`, GitHub Release 제목은 `AutoSRT 2026.10.0` 형식을 사용한다.
- 시험판은 다음 정식 버전에 Cargo/SemVer 접미사 `-rcN`을 붙인다. 예: `2026.10.2-rc1`, `2026.10.2-rc2` → 정식 `2026.10.2`. 시험판의 로컬 빌드·설치는 GitHub 릴리스나 Homebrew 배포를 자동으로 의미하지 않는다.
- Cargo.toml, Cargo.lock, 실행파일 `--version`, 태그, 릴리스 자산과 Homebrew formula 버전이 같아야 한다.
- CalVer는 API 호환성을 보장하지 않는다. 호환성 변경과 이전 절차는 릴리스 노트에 명시한다.
- 게시한 태그와 자산은 덮어쓰지 않는다. 오류를 고친 릴리스는 PATCH를 올려 새로 게시한다.

## 저장소

애플리케이션은 `https://github.com/yoophi/autosrt`, Homebrew tap은 `https://github.com/yoophi/homebrew-tap`이다. Homebrew 이름은 `yoophi/tap/autosrt`다.

첫 릴리스의 사전 빌드 바이너리는 macOS 15 이상 Apple Silicon(aarch64-apple-darwin)용이다. Linux·Intel macOS 바이너리는 검증한 빌드 환경을 추가하기 전까지 제공하지 않는다. 모델은 크기와 선택권을 고려해 바이너리 자산에 포함하지 않는다. Homebrew는 ffmpeg, yt-dlp, whisper.cpp를 의존성으로 설치한다.

## 배포 절차

1. 게시일 기준으로 다음 버전을 결정하고 Cargo.toml의 package.version을 수정한다. `cargo check`로 Cargo.lock을 갱신한다.
2. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked`를 통과시킨다. 릴리스 노트에 지원 플랫폼과 알려진 제한을 기록한다.
3. `scripts/package-release.sh`로 릴리스 바이너리, 라이선스, README를 묶고 SHA256SUMS를 생성한다. 결과물은 git에서 제외되는 `dist/`에 생성한다.
4. 바이너리 `--version`, 설정 명령과 doctor를 검증한다. 사용자 데이터·모델·설정 파일이 배포물에 포함되지 않았는지 tar 목록을 확인한다.
5. 변경 사항을 커밋하고 main에 푸시한다. 검사한 커밋에 annotated tag `v<VERSION>`을 만들고 해당 태그를 푸시한다.
6. `gh release create v<VERSION> dist/<archive> dist/SHA256SUMS --verify-tag --title "AutoSRT <VERSION>" --notes-file <notes>`로 릴리스를 생성한다. GitHub에 올라간 자산을 다시 받아 SHA256을 비교한다.
7. tap의 `Formula/autosrt.rb`에 새 버전의 릴리스 URL과 SHA256을 기록한다. Ruby 문법·Homebrew style 검사 후 커밋하고 푸시한다. 다른 formula는 변경하지 않는다.
8. `brew install yoophi/tap/autosrt` 또는 기존 설치의 `brew upgrade yoophi/tap/autosrt`로 실제 공개 URL 설치를 검증한다. `brew test yoophi/tap/autosrt`와 설치된 실행파일 `--version`, `doctor`를 확인한다.
9. 배포 검증이 완료되면 td 작업과 완료 알림에 릴리스 URL을 기록한다. 실패한 단계가 있으면 배포 완료로 보고하지 않는다.

## 롤백

기존 태그나 자산을 수정하지 않는다. 문제가 확인되면 릴리스 노트에 제한을 추가하고 수정 PATCH를 배포한다. 필요하면 사용자는 이전 릴리스의 바이너리와 SHA256을 확인하여 직접 설치한다. Homebrew formula를 이전 버전으로 돌리는 작업은 별도로 명시하고 검증한다.
