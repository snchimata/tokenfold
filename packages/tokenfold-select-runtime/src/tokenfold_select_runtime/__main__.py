import sys
if __name__ == '__main__':
    if len(sys.argv)>1 and sys.argv[1]=='provision':
        from .provision import main
        main(sys.argv[2:])
    elif sys.argv[1:]==['worker']:
        from .worker import main
        raise SystemExit(main())
    else:
        raise SystemExit('expected provision or worker')
