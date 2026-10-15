_pgloupe() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="pgloupe"
                ;;
            pgloupe,doctor)
                cmd="pgloupe__subcmd__doctor"
                ;;
            pgloupe,export)
                cmd="pgloupe__subcmd__export"
                ;;
            pgloupe,help)
                cmd="pgloupe__subcmd__help"
                ;;
            pgloupe,logs)
                cmd="pgloupe__subcmd__logs"
                ;;
            pgloupe,report)
                cmd="pgloupe__subcmd__report"
                ;;
            pgloupe,snap)
                cmd="pgloupe__subcmd__snap"
                ;;
            pgloupe,stats)
                cmd="pgloupe__subcmd__stats"
                ;;
            pgloupe,watch)
                cmd="pgloupe__subcmd__watch"
                ;;
            pgloupe__subcmd__help,doctor)
                cmd="pgloupe__subcmd__help__subcmd__doctor"
                ;;
            pgloupe__subcmd__help,export)
                cmd="pgloupe__subcmd__help__subcmd__export"
                ;;
            pgloupe__subcmd__help,help)
                cmd="pgloupe__subcmd__help__subcmd__help"
                ;;
            pgloupe__subcmd__help,logs)
                cmd="pgloupe__subcmd__help__subcmd__logs"
                ;;
            pgloupe__subcmd__help,report)
                cmd="pgloupe__subcmd__help__subcmd__report"
                ;;
            pgloupe__subcmd__help,snap)
                cmd="pgloupe__subcmd__help__subcmd__snap"
                ;;
            pgloupe__subcmd__help,stats)
                cmd="pgloupe__subcmd__help__subcmd__stats"
                ;;
            pgloupe__subcmd__help,watch)
                cmd="pgloupe__subcmd__help__subcmd__watch"
                ;;
            pgloupe__subcmd__help__subcmd__logs,analyze)
                cmd="pgloupe__subcmd__help__subcmd__logs__subcmd__analyze"
                ;;
            pgloupe__subcmd__help__subcmd__logs,dump)
                cmd="pgloupe__subcmd__help__subcmd__logs__subcmd__dump"
                ;;
            pgloupe__subcmd__help__subcmd__logs,fetch)
                cmd="pgloupe__subcmd__help__subcmd__logs__subcmd__fetch"
                ;;
            pgloupe__subcmd__help__subcmd__logs,ingest)
                cmd="pgloupe__subcmd__help__subcmd__logs__subcmd__ingest"
                ;;
            pgloupe__subcmd__help__subcmd__logs,report)
                cmd="pgloupe__subcmd__help__subcmd__logs__subcmd__report"
                ;;
            pgloupe__subcmd__help__subcmd__stats,diff)
                cmd="pgloupe__subcmd__help__subcmd__stats__subcmd__diff"
                ;;
            pgloupe__subcmd__logs,analyze)
                cmd="pgloupe__subcmd__logs__subcmd__analyze"
                ;;
            pgloupe__subcmd__logs,dump)
                cmd="pgloupe__subcmd__logs__subcmd__dump"
                ;;
            pgloupe__subcmd__logs,fetch)
                cmd="pgloupe__subcmd__logs__subcmd__fetch"
                ;;
            pgloupe__subcmd__logs,help)
                cmd="pgloupe__subcmd__logs__subcmd__help"
                ;;
            pgloupe__subcmd__logs,ingest)
                cmd="pgloupe__subcmd__logs__subcmd__ingest"
                ;;
            pgloupe__subcmd__logs,report)
                cmd="pgloupe__subcmd__logs__subcmd__report"
                ;;
            pgloupe__subcmd__logs__subcmd__help,analyze)
                cmd="pgloupe__subcmd__logs__subcmd__help__subcmd__analyze"
                ;;
            pgloupe__subcmd__logs__subcmd__help,dump)
                cmd="pgloupe__subcmd__logs__subcmd__help__subcmd__dump"
                ;;
            pgloupe__subcmd__logs__subcmd__help,fetch)
                cmd="pgloupe__subcmd__logs__subcmd__help__subcmd__fetch"
                ;;
            pgloupe__subcmd__logs__subcmd__help,help)
                cmd="pgloupe__subcmd__logs__subcmd__help__subcmd__help"
                ;;
            pgloupe__subcmd__logs__subcmd__help,ingest)
                cmd="pgloupe__subcmd__logs__subcmd__help__subcmd__ingest"
                ;;
            pgloupe__subcmd__logs__subcmd__help,report)
                cmd="pgloupe__subcmd__logs__subcmd__help__subcmd__report"
                ;;
            pgloupe__subcmd__stats,diff)
                cmd="pgloupe__subcmd__stats__subcmd__diff"
                ;;
            pgloupe__subcmd__stats,help)
                cmd="pgloupe__subcmd__stats__subcmd__help"
                ;;
            pgloupe__subcmd__stats__subcmd__help,diff)
                cmd="pgloupe__subcmd__stats__subcmd__help__subcmd__diff"
                ;;
            pgloupe__subcmd__stats__subcmd__help,help)
                cmd="pgloupe__subcmd__stats__subcmd__help__subcmd__help"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        pgloupe)
            opts="-h -V --help --version stats logs report snap doctor export watch help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__doctor)
            opts="-h --conn --password-env --sslrootcert --vendor --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --conn)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --password-env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sslrootcert)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --vendor)
                    COMPREPLY=($(compgen -W "rds" -- "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__export)
            opts="-h --conn --password-env --sslrootcert --provider --instance --region --profile --project --resource-group --subscription --log-group --bin --since --out --report --prefix --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --conn)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --password-env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sslrootcert)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --provider)
                    COMPREPLY=($(compgen -W "aws-rds aws-cloudwatch azure-flexible gcp-cloudsql aiven heroku sql" -- "${cur}"))
                    return 0
                    ;;
                --instance)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --region)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --profile)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --resource-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --subscription)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bin)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --since)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --report)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prefix)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help)
            opts="stats logs report snap doctor export watch help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__doctor)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__export)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__logs)
            opts="dump analyze ingest report fetch"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__logs__subcmd__analyze)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__logs__subcmd__dump)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__logs__subcmd__fetch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__logs__subcmd__ingest)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__logs__subcmd__report)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__report)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__snap)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__stats)
            opts="diff"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__stats__subcmd__diff)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__help__subcmd__watch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs)
            opts="-h --help dump analyze ingest report fetch help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__analyze)
            opts="-h --format --prefix --log-tz --by --top --bucket --max-fingerprints --threads --output --slowest --params --database --user --application --host --exclude-database --exclude-user --exclude-application --exclude-host --include-query --exclude-query --pid --exclude-message --between --exclude-hours --weekdays --tz --anonymize --anonymize-key --lc-messages --messages-file --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --format)
                    COMPREPLY=($(compgen -W "auto csv json stderr pgbouncer" -- "${cur}"))
                    return 0
                    ;;
                --prefix)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --by)
                    COMPREPLY=($(compgen -W "total calls p95 max" -- "${cur}"))
                    return 0
                    ;;
                --top)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bucket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --max-fingerprints)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --threads)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                --slowest)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --database)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --user)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --application)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-database)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-user)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-application)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --include-query)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-query)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pid)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-message)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --between)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-hours)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --weekdays)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --anonymize-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lc-messages)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --messages-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__dump)
            opts="-h --format --prefix --log-tz --limit --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --format)
                    COMPREPLY=($(compgen -W "auto csv json stderr pgbouncer" -- "${cur}"))
                    return 0
                    ;;
                --prefix)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --limit)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__fetch)
            opts="-h --provider --instance --region --profile --project --resource-group --subscription --log-group --bin --conn --password-env --sslrootcert --since --out --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --provider)
                    COMPREPLY=($(compgen -W "aws-rds aws-cloudwatch azure-flexible gcp-cloudsql aiven heroku sql" -- "${cur}"))
                    return 0
                    ;;
                --instance)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --region)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --profile)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --resource-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --subscription)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bin)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --conn)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --password-env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sslrootcert)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --since)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__help)
            opts="dump analyze ingest report fetch help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__help__subcmd__analyze)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__help__subcmd__dump)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__help__subcmd__fetch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__help__subcmd__ingest)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__help__subcmd__report)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__ingest)
            opts="-h --store --format --prefix --log-tz --max-fingerprints --threads --force --slowest --params --database --user --application --host --exclude-database --exclude-user --exclude-application --exclude-host --include-query --exclude-query --pid --exclude-message --between --exclude-hours --weekdays --tz --anonymize --anonymize-key --lc-messages --messages-file --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --store)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "auto csv json stderr pgbouncer" -- "${cur}"))
                    return 0
                    ;;
                --prefix)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --max-fingerprints)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --threads)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --slowest)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --database)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --user)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --application)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-database)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-user)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-application)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --include-query)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-query)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pid)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-message)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --between)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-hours)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --weekdays)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --anonymize-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lc-messages)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --messages-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__logs__subcmd__report)
            opts="-h --store --slowest --since --until --by --top --bucket --output --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --store)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --slowest)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --since)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --until)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --by)
                    COMPREPLY=($(compgen -W "total calls p95 max" -- "${cur}"))
                    return 0
                    ;;
                --top)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bucket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__report)
            opts="-h --store --since --until --before --after --before-info --after-info --before-db --after-db --format --prefix --log-tz --max-fingerprints --threads --by --top --bucket --output --baseline --baseline-since --baseline-until --settings --vendor --display-tz --out --slowest --params --database --user --application --host --exclude-database --exclude-user --exclude-application --exclude-host --include-query --exclude-query --pid --exclude-message --between --exclude-hours --weekdays --tz --anonymize --anonymize-key --lc-messages --messages-file --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --store)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --since)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --until)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --before)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --after)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --before-info)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --after-info)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --before-db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --after-db)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "auto csv json stderr pgbouncer" -- "${cur}"))
                    return 0
                    ;;
                --prefix)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --max-fingerprints)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --threads)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --by)
                    COMPREPLY=($(compgen -W "total calls p95 max" -- "${cur}"))
                    return 0
                    ;;
                --top)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bucket)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output)
                    COMPREPLY=($(compgen -W "text json html csv" -- "${cur}"))
                    return 0
                    ;;
                --baseline)
                    COMPREPLY=($(compgen -W "previous" -- "${cur}"))
                    return 0
                    ;;
                --baseline-since)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --baseline-until)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --settings)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --vendor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --display-tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --slowest)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --database)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --user)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --application)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-database)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-user)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-application)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-host)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --include-query)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-query)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pid)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-message)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --between)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --exclude-hours)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --weekdays)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tz)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --anonymize-key)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --lc-messages)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --messages-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__snap)
            opts="-h --conn --password-env --sslrootcert --out --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --conn)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --password-env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sslrootcert)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__stats)
            opts="-h --help diff help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__stats__subcmd__diff)
            opts="-h --before-info --after-info --by --top --min-calls --wide --format --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --before-info)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --after-info)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --by)
                    COMPREPLY=($(compgen -W "total calls mean" -- "${cur}"))
                    return 0
                    ;;
                --top)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --min-calls)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__stats__subcmd__help)
            opts="diff help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__stats__subcmd__help__subcmd__diff)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__stats__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        pgloupe__subcmd__watch)
            opts="-h --conn --password-env --sslrootcert --provider --instance --region --profile --project --resource-group --subscription --log-group --bin --interval --window --listen --allow-remote --state-dir --keep-snapshots --prefix --out --once --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --conn)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --password-env)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --sslrootcert)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --provider)
                    COMPREPLY=($(compgen -W "aws-rds aws-cloudwatch azure-flexible gcp-cloudsql aiven heroku sql" -- "${cur}"))
                    return 0
                    ;;
                --instance)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --region)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --profile)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --project)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --resource-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --subscription)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --log-group)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bin)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --interval)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --window)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --listen)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --state-dir)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --keep-snapshots)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --prefix)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _pgloupe -o nosort -o bashdefault -o default pgloupe
else
    complete -F _pgloupe -o bashdefault -o default pgloupe
fi
