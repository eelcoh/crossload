/*
  Copyright 2021 Grégory Soutadé

  This file is part of uPDFParser.

  uPDFParser is free software: you can redistribute it and/or modify
  it under the terms of the GNU Lesser General Public License as published by
  the Free Software Foundation, either version 3 of the License, or
  (at your option) any later version.

  uPDFParser is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU Lesser General Public License for more details.

  You should have received a copy of the GNU Lesser General Public License
  along with uPDFParser. If not, see <http://www.gnu.org/licenses/>.
*/

#include <sys/types.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>

#include "uPDFParser.h"
#include "uPDFParser_common.h"

namespace uPDFParser
{
    std::string Object::str()
    {
	std::stringstream res;

	res << _objectId << " " << _generationNumber << " obj\n";
	if (isIndirect())
	    res << "   " << indirectOffset << "\n";
	else
	{
	    bool needLineReturn = false;
	    
	    if (!_dictionary.empty())
		res << _dictionary.str();
	    else
	    {
		if (!_data.size())
		    res << "<<>>\n";
		else
		    needLineReturn = true;
	    }

	    std::vector<DataType*>::iterator it;
	    for(it=_data.begin(); it!=_data.end(); it++)
	    {
		std::string tmp = (*it)->str();
		res << tmp;
		if (tmp[tmp.size()-1] == '\n' ||
		    tmp[tmp.size()-1] == '\r')
		    needLineReturn = false;
	    }

	    if (needLineReturn)
		res << "\n";
	}

	res << "endobj\n";

	return res.str();
    }

    static DataType* tokenToNumber(std::string& token, char sign='\0')
    {
	int i;
	float fvalue;
	int ivalue;
	
	for(i=0; i<(int)token.size(); i++)
	{
	    if (token[i] == '.')
	    {
		if (i==0) token = std::string("0") + token;
		fvalue = std::stof(token);
		if (sign == '-')
		    fvalue = -fvalue;
		return new Real(fvalue, (sign!='\0'));
	    }
	}

	ivalue = std::stoi(token);
	if (sign == '-')
	    ivalue = -ivalue;
	
	return new Integer(ivalue, (sign!='\0'));
    }

    /**
     * @brief Read data until '\n' or '\r' is found or buffer is full
     */
    static inline int readline(int fd, char* buffer, int size, bool exceptionOnEOF=true)
    {
	int res = 0;
	char c;

	buffer[0] = 0;
	
	for (;size;size--,res++)
	{
	    if (read(fd, &c, 1) != 1)
	    {
		if (exceptionOnEOF)
		    EXCEPTION(TRUNCATED_FILE, "Unexpected end of file");
		return -1;
	    }

	    if (c == '\n' || c == '\r')
	    {
		// Empty line
		if (!res)
		{
		    size++ ;
		    res--;
		    continue;
		}
		else
		    break;
	    }

	    buffer[res] = c;
	}

	if (size)
	    buffer[res] = 0;
	
	return res;
    }

    /**
     * @brief Read data until EOF, '\n' or '\r' is found
     */
    static inline void finishLine(int fd)
    {
	char c;
	
	while (1)
	{
	    if (read(fd, &c, 1) != 1)
		break;

	    if (c == '\n' || c == '\r')
		break;
	}
	// Support \r\n and \n\r
	if (read(fd, &c, 1) == 1)
	{
	    if (c != '\n' && c != '\r')
		lseek(fd, -1, SEEK_CUR);
	}
    }

    char Parser::prevChar() { return c; }
    
    /**
     * @brief Find next token to analyze
     */
    std::string Parser::nextToken(bool exceptionOnEOF, bool readComment)
    {
	char prev_c;
	std::string res("");
	int i;
	static const char delims[] = " \t<>[]()/";
	static const char whitespace_prev_delims[] = "+-"; // Need whitespace before
	static const char start_delims[] = "<>[]()";
	bool found = false;

	c = 0;
	while (!found)
	{
	    prev_c = c;
	    if (read(fd, &c, 1) != 1)
	    {
		if (exceptionOnEOF)
		    EXCEPTION(TRUNCATED_FILE, "Unexpected end of file");
		break;
	    }

	    // Comment, skip line
	    if (c == '%')
	    {
		if (readComment)
		{
		    curOffset = lseek(fd, 0, SEEK_CUR)-1;
		    res += c;
		    while (true)
		    {
			if (read(fd, &c, 1) != 1)
			{
			    if (exceptionOnEOF)
				EXCEPTION(TRUNCATED_FILE, "Unexpected end of file");
			    break;
			}
			if (c == '\n' || c == '\r')
			    break;
			res += c;
		    }
		    break;
		}
		
		finishLine(fd);
		if (res.size())
		    break;
		else
		    continue;
	    }

	    // White character while empty result, continue
	    if ((c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\0') && !res.size())
		continue;

	    // Quit on line return without lseek(fd, -1, SEEK_CUR)
	    if (c == '\n' || c == '\r')
	    {
		if (res.size())
		    break;
		else
		    continue;
	    }

	    if (res.size())
	    {
		// Push character until delimiter is found
		for (i=0; i<(int)sizeof(delims); i++)
		{
		    if (c == delims[i])
		    {
			lseek(fd, -1, SEEK_CUR);
			found = true;
			break;
		    }
		}
		
		// Push character until delimiter is found
		if (!found && prev_c == ' ')
		{
		    for (i=0; i<(int)sizeof(whitespace_prev_delims); i++)
		    {
			if (c == whitespace_prev_delims[i])
			{
			    lseek(fd, -1, SEEK_CUR);
			    found = true;
			    break;
			}
		    }
		}
		
		if (!found)
		    res += c;
	    }
	    else
	    {
		curOffset = lseek(fd, 0, SEEK_CUR)-1;

		// First character, is it a delimiter ?
		for (i=0; i<(int)sizeof(start_delims); i++)
		{
		    if (c == start_delims[i])
		    {
			found = true;
			break;
		    }
		}

		res += c;
	    }
	}

	// Double '>' and '<' to compute dictionary
	if (res == ">" || res == "<")
	{
	    if (read(fd, &c, 1) == 1)
	    {
		if (c == res[0])
		    res += c;
		else
		    lseek(fd, -1, SEEK_CUR);
	    }
	}
	
	return res;
    }

    void Parser::parseHeader()
    {
	char buf[5];

	// Check %PDF at startup
	readline(fd, buf, 5, false);
	if (strncmp(buf, "%PDF-", 5))
	    EXCEPTION(INVALID_HEADER, "Invalid PDF header");

	// Read major
	readline(fd, buf, 1, false);
	if (buf[0] < '0' || buf[0] > '9')
	    EXCEPTION(INVALID_HEADER, "Invalid PDF major version " << buf[0]);
	version_major = buf[0] - '0';
	
	readline(fd, buf, 1, false);
	if (buf[0] != '.')
	    EXCEPTION(INVALID_HEADER, "Invalid PDF header");

	// Read minor
	readline(fd, buf, 1, false);
	if (buf[0] < '0' || buf[0] > '9')
	    EXCEPTION(INVALID_HEADER, "Invalid PDF minor version " << buf[0]);
	version_minor = buf[0] - '0';

	finishLine(fd);
	curOffset = lseek(fd, 0, SEEK_CUR);
    }
    
    void Parser::parseStartXref()
    {
	std::string offset, token;

	// std::cout << "Parse startxref" << std::endl;

	offset = nextToken(); // XREF offset
	token = nextToken(false, true); // %%EOF
	if (strncmp(token.c_str(), "%%EOF", 5))
	    EXCEPTION(INVALID_TRAILER, "Invalid trailer at offset " << curOffset);
	/* 
	   Handle special case where we have :
	   %%EOF1 0 obj\n
	 */
	if (token.size() > 5)
	    lseek(fd, curOffset+5, SEEK_SET);

	/* Case where no xref table present */
	if (xrefOffset == (off_t)-1)
	{
	    DataType* integer = tokenToNumber(offset);
	    if (integer->type() != DataType::TYPE::INTEGER)
		EXCEPTION(INVALID_TRAILER, "Invalid startxref offset");

	    xrefOffset = ((Integer*)integer)->value();
	}
		
    }
    
    bool Parser::parseTrailer()
    {
	std::string token;

	// std::cout << "Parse trailer" << std::endl;

	token = nextToken();

	if (token != "<<")
	    EXCEPTION(INVALID_TRAILER, "Invalid trailer at offset " << curOffset);

	parseDictionary(&trailer, trailer.dictionary().value());

	token = nextToken();
	/* trailer without xref */
	if (token != "startxref")
	{
	    lseek(fd, curOffset, SEEK_SET);
	    return false;
	}

	parseStartXref();
	return true;
    }
    
    bool Parser::parseXref()
    {
	std::string tokens[3];
	bool res = false;
	int curId = 0;
	
	// std::cout << "Parse xref" << std::endl;
	xrefOffset = curOffset;

	while (1)
	{
	    tokens[0] = nextToken();

	    if (tokens[0] == "trailer")
		break;

	    tokens[1] = nextToken();

	    // Reference ie: 0000000016 00000 n
	    if (tokens[0].length() == 10)
	    {
		tokens[2] = nextToken();
		XRefValue xref(curId, std::stoi(tokens[0],0,10), std::stoi(tokens[1],0,10),
			       (tokens[2] == "n") ? true : false);
		_xrefTable.push_back(xref);
		curId++;

	    }
	    // Object index ie: 0 6121
	    else
	    {
		curId = std::stoi(tokens[0]);
	    }
	}

	res = parseTrailer();
	return res;
    }
    
    DataType* Parser::parseSignedNumber(std::string& token)
    {
	char sign = token[0];
	token = std::string(&((token.c_str())[1]));
	return tokenToNumber(token, sign);
    }
    
    DataType* Parser::parseNumber(std::string& token)
    {
	return tokenToNumber(token);
    }

    DataType* Parser::parseNumberOrReference(std::string& token)
    {
	DataType* res = tokenToNumber(token);

	if (res->type() == DataType::TYPE::REAL)
	    return res;
	
	off_t offset = lseek(fd, 0, SEEK_CUR);
	std::string token2 = nextToken();
	std::string token3 = nextToken();

	DataType* generationNumber = 0;
	try
	{
	    generationNumber = tokenToNumber(token2);
	}
	catch (std::invalid_argument& e)
	{
	    lseek(fd, offset, SEEK_SET);
	    return res;
	}
	
	if ((generationNumber->type() != DataType::TYPE::INTEGER) ||
	    token3.size() != 1 || token3[0] != 'R')
	{
	    delete generationNumber;
	    lseek(fd, offset, SEEK_SET);
	    return res;
	}

	DataType* res2 = new Reference(((Integer*)res)->value(),
				       ((Integer*)generationNumber)->value());
	delete res;
	return res2;
    }
    
    DataType* Parser::parseType(std::string& token, Object* object, std::map<std::string, DataType*>& dict)
    {
	DataType* value = 0;
	Dictionary* _value = 0;

	if (token == "<<")
	{
	    _value = new Dictionary();
	    value = _value;
	    parseDictionary(object, _value->value());
	}
	else if (token == "[")
	    value = parseArray(object);
	else if (token == "(")
	    value = parseString();
	else if (token == "<")
	    value = parseHexaString();
	else if (token == "stream")
	    value = parseStream(object);
	else if (token[0] >= '1' && token[0] <= '9')
	    value = parseNumberOrReference(token);
	else if (token[0] == '/')
	    value = parseName(token);
	else if (token[0] == '+' || token[0] == '-')
	    value = parseSignedNumber(token);
	else if (token[0] == '0' || token[0] == '.')
	    value = parseNumber(token);
	else if (token == "true")
	    return new Boolean(true);
	else if (token == "false")
	    return new Boolean(false);
	else if (token == "null")
	    return new Null();
	else
	    EXCEPTION(INVALID_TOKEN, "Invalid token " << token << " at offset " << curOffset);

	return value;
    }

    Array* Parser::parseArray(Object* object)
    {
	std::string token;
	DataType* value;

	Array* res = new Array();
	
	while (1)
	{
	    token = nextToken();

	    if (token == "]")
		break;

	    value = parseType(token, object, object->dictionary().value());
	    //std::cout << "Add " << value->str() << std::endl;
	    res->addData(value);
	}

	return res;
    }
    
    String* Parser::parseString()
    {
	std::string res("");
	char c;
	bool escaped = false;
	int parenthesis_count = 1; /* Handle parenthesis in parenthesis */
	
	while (1)
	{
	    if (read(fd, &c, 1) != 1)
		break;

	    if (c == '(' && !escaped)
		parenthesis_count++;
	    else if (c == ')' && !escaped)
		parenthesis_count--;
		
	    if (c == ')' && !escaped && parenthesis_count == 0)
		break;

	    /* Handle \\ */
	    if (c == '\\' && escaped)
		escaped = false;
	    else
		escaped = (c == '\\');

	    res += c;
	}

	return new String(res);
    }
    
    HexaString* Parser::parseHexaString()
    {
	std::string res("");
	char c;
	
	while (1)
	{
	    if (read(fd, &c, 1) != 1)
		break;

	    if (c == '>')
		break;

	    res += c;
	}

	if ((res.size() % 2))
	    EXCEPTION(INVALID_HEXASTRING, "Invalid hexa String at offset " << curOffset);
	    
	return new HexaString(res);
    }

    Stream* Parser::parseStream(Object* object)
    {
	off_t startOffset, endOffset, endStream;
	std::string token;
	char c = 0;
	
	// std::cout << "parseStream" << std::endl;
	
	// Remove \n after \r if there is one
	if (prevChar() == '\r' && read(fd, &c, 1) == 1)
	{
	    if (c != '\n')
	    {
		lseek(fd, -1, SEEK_CUR);
	    }
	}

	startOffset = lseek(fd, 0, SEEK_CUR);

	if (!object->hasKey("Length"))
	    EXCEPTION(INVALID_STREAM, "No Length property at offset " << curOffset);

	DataType* Length = (*object)["Length"];
	if (Length->type() == DataType::INTEGER)
	{
	    Integer* length = (Integer*)Length;
	    endOffset = startOffset + length->value();
	    lseek(fd, endOffset, SEEK_SET);
	    token = nextToken();

	    if (token == "endstream")
		return new Stream(object->dictionary(), startOffset, endOffset,
				  0, 0, false, fd);

	    // No endstream, come back at the begining
	    lseek(fd, startOffset, SEEK_SET);
	}
	
	// Don't want to parse xref table...
	while (1)
	{
	    char buffer[4*1024];
	    char* subs, c;
	    int ret;
	    ret = read(fd, buffer, sizeof(buffer));
	    if (ret <= 0)
		EXCEPTION(TRUNCATED_FILE, "Unexpected end of file");
	    subs = (char*)memmem((void*)buffer, ret, (void*)"endstream", 9);
	    if (subs)
	    {
		unsigned long pos = (unsigned long)subs - (unsigned long)buffer;
		// Here we're juste before "enstream"
		endOffset = lseek(fd, -(ret-pos), SEEK_CUR);
		// Final position must be after endstream\n
		endStream = endOffset + 10;
		// Remove trailing \r and \n before endstream
		for (;endOffset > startOffset; endOffset--)
		{
		    lseek(fd, -1, SEEK_CUR);
		    ret = read(fd, &c, 1);
		    if (ret <= 0)
			break;
		    if (c == '\r')
		    {
			lseek(fd, -1, SEEK_CUR);
			continue;
		    }
		    else if (c == '\n')
		    {
			lseek(fd, -1, SEEK_CUR);
		    }
		    break;
		}
		// Adjust final position
		lseek(fd, endStream, SEEK_SET);
		break;
	    }
	}
	
	return new Stream(object->dictionary(), startOffset, endOffset,
			  0, 0, false, fd);
    }
    
    Name* Parser::parseName(std::string& name)
    {
	if (!name.size() || name[0] != '/')
	    EXCEPTION(INVALID_NAME, "Invalid Name at offset " << curOffset);

	//std::cout << "Name " << name << std::endl;
	return new Name(name);
    }
   
    void Parser::parseDictionary(Object* object, std::map<std::string, DataType*>& dict)
    {
	std::string token;
	Name* key;
	DataType* value;

	while (1)
	{
	    token = nextToken();
	    if (token == ">>")
		break;

	    key = parseName(token);

	    token = nextToken();
	    if (token == ">>")
	    {
		dict[key->value()] = 0;
		break;
	    }

	    value = parseType(token, object, dict);
	    dict[key->value()] = value;
	}
    }
    
    void Parser::parseObject(std::string& token)
    {
	off_t offset;
	int objectId, generationNumber;
	Object* object;

	offset = curOffset;
	try
	{
	    objectId = std::stoi(token);
	    token = nextToken();
	    generationNumber = std::stoi(token);
	}
	catch(std::invalid_argument& e)
	{
	    EXCEPTION(INVALID_OBJECT, "Invalid object at offset " << curOffset);
	}

	token = nextToken();

	if (token != "obj")
	    EXCEPTION(INVALID_OBJECT, "Invalid object at offset " << curOffset);

	// std::cout << "New obj " << objectId << " " << generationNumber << std::endl;
	
	object = new Object(objectId, generationNumber, offset);
	_objects.push_back(object);
	std::vector<DataType*>& datas = object->data();
	
	while (1)
	{
	    token = nextToken();

	    if (token == "endobj")
		break;

	    if (token == "<<")
		parseDictionary(object, object->dictionary().value());
	    else if (token[0] >= '1' && token[0] <= '9')
	    {
		DataType* _offset = tokenToNumber(token);
		if (_offset->type() != DataType::TYPE::INTEGER)
		    EXCEPTION(INVALID_OBJECT, "Invalid object at offset " << curOffset);
		object->setIndirectOffset(((Integer*)_offset)->value());
	    }
	    else
	    {
		DataType* res = parseType(token, object, object->dictionary().value());
		datas.push_back(res);
	    }
	}

	// Keep a reference to last xrefObject
	if (object->hasKey("Type") && (*object)["Type"]->str() == "/XRef")
	    xrefObject = object;
    }

    void Parser::parse(const std::string& filename)
    {
	std::string token;
	bool secondLine = true;
	
	if (fd)
	    close(fd);

	fd = open(filename.c_str(), O_RDONLY);
	
	if (fd <= 0)
	    EXCEPTION(UNABLE_TO_OPEN_FILE, "Unable to open " << filename << " (%m)");

	parseHeader();
	
	// // Check %%EOF at then end
	// lseek(fd, -5, SEEK_END);
	// readline(fd, buf, 5);
	// if (strncmp(buf, "%%EOF", 5))
	//     EXCEPTION(INVALID_FOOTER, "Invalid PDF footer");

	lseek(fd, curOffset, SEEK_SET);

	while (1)
	{
	    token = nextToken(false);

	    if (!token.size())
		break;

	    if (token == "xref")
		parseXref();
	    else if (token[0] >= '1' && token[0] <= '9')
		parseObject(token);
	    // Can have startxref without trailer (not end of document)
	    else if (token == "startxref")
		parseStartXref();
	    else
	    {
		// The second line may be not commented and invalid (for UTF8 stuff)
		if (!secondLine)
		{
		    EXCEPTION(INVALID_LINE, "Invalid Line at offset " << curOffset);
		}
		else
		    finishLine(fd);
	    }
	    // If for optimization
	    if (secondLine) secondLine = false;
	}

	// Synchronize xref table with parsed objects
	std::vector<XRefValue>::iterator it;
	for (it=_xrefTable.begin(); it != _xrefTable.end(); it++)
	{
	    Object* object = getObject((*it).objectId(), (*it).generationNumber());
	    if (object)
	    {
		(*it).setObject(object);
		object->setUsed((*it).used());
	    }
	}

	repairTrailer();
	
	// close(fd);
    }

    Object* Parser::getObject(int objectId, int generationNumber)
    {
	std::vector<Object*>::iterator it;

	Object object(objectId, generationNumber, 0);
	
	for (it = _objects.begin(); it != _objects.end(); it++)
	{
	    if (**it == object)
		return *it;
	}

	return 0;
    }

    void Parser::repairTrailer()
    {
	// Try to fill manadatory values not present in original trailer
	// with xrefObject if there is one
	if (!xrefObject)
	    return;

	static const char* keys[] = {"Root", "Info", "Encrypt", "ID"};

	for (int i=0; i<(int)(sizeof(keys)/sizeof(keys[0])); i++)
	{
	    if (!trailer.hasKey(keys[i]) && xrefObject->hasKey(keys[i]))
		trailer.dictionary().addData(keys[i], (*xrefObject)[keys[i]]->clone());
	}
    }

    void Parser::removeObject(Object* object)
    {
	std::vector<Object*>::iterator it;

	for(it = _objects.begin(); it != _objects.end(); it++)
	{
	    if (**it == *object)
	    {
		delete *it;
		_objects.erase(it);
		break;
	    }
	}	
    }
    
    void Parser::writeBuffer(int fd, const char* buffer, int size)
    {
	int ret;

	do {
	    ret = ::write(fd, buffer, size);
	    if (ret >= 0)
	    {
		size -= ret;
		buffer += ret;
	    }
	    else
	    {
		EXCEPTION(IO_ERROR, "IO Error (write) %m");
	    }
	} while (size);
    }
    
    void Parser::writeUpdate(const std::string& filename)
    {
	struct stat _stat;

	int statRet = stat(filename.c_str(), &_stat);

	int newFd = open(filename.c_str(), O_WRONLY|O_APPEND|O_CREAT, S_IRUSR|S_IWUSR);

	if (newFd <= 0)
	    EXCEPTION(UNABLE_TO_OPEN_FILE, "Unable to open " << filename << " (%m)");

	// Copy file if it doesn't exists
	if (statRet == -1 && errno == ENOENT)
	{
	    char buffer[4096];
	    int ret;
	    lseek(fd, 0, SEEK_SET);

	    while (true)
	    {
		ret = ::read(fd, buffer, sizeof(buffer));
		if (ret <= 0)
		    break;
		writeBuffer(newFd, buffer, ret);
	    }
	}
	
	writeBuffer(newFd, "\r", 1);

	int maxId = 0;
	std::stringstream xref;
	int nbNewObjects = 0;

	xref << std::setfill('0');
	xref << "xref\n";
	
	std::vector<Object*>::iterator it;
	for(it=_objects.begin(); it!=_objects.end(); it++)
	{
	    Object* object = *it;
	    if (object->objectId() > maxId)
		maxId = object->objectId();
	    if (!object->isNew())
		continue;
	    nbNewObjects ++;
	    std::string objStr = object->str();
	    curOffset = lseek(newFd, 0, SEEK_CUR);
	    writeBuffer(newFd, objStr.c_str(), objStr.size());
	    xref << std::setw(0) << object->objectId() << " 1\n";
	    xref << std::setw(10) << curOffset << " " << std::setw(5) << object->generationNumber() << " n\r\n"; // Here \r seems important 
	}

	if (!nbNewObjects)
	{
	    close(newFd);
	    return;
	}

	off_t newXrefOffset = lseek(newFd, 0, SEEK_CUR);

	std::string xrefStr = xref.str();
	writeBuffer(newFd, xrefStr.c_str(), xrefStr.size());

	trailer.deleteKey("Prev");
	if (xrefOffset != (off_t)-1)
	    trailer.dictionary().addData("Prev", new Integer((int)xrefOffset));
	trailer.deleteKey("Size");
	trailer.dictionary().addData("Size", new Integer(maxId+1));

	std::string trailerStr = trailer.dictionary().str();
	writeBuffer(newFd, "trailer\n", 8);
	writeBuffer(newFd, trailerStr.c_str(), trailerStr.size());

	std::stringstream startxref;
	startxref << "startxref\n" << newXrefOffset << "\n%%EOF";
	
	std::string startxrefStr = startxref.str();
	writeBuffer(newFd, startxrefStr.c_str(), startxrefStr.size());
	
	close(newFd);
    }
    
    void Parser::write(const std::string& filename, bool update)
    {
	if (update)
	    return writeUpdate(filename);

	int newFd = open(filename.c_str(), O_WRONLY|O_CREAT|O_TRUNC, S_IRUSR|S_IWUSR);

	if (newFd <= 0)
	    EXCEPTION(UNABLE_TO_OPEN_FILE, "Unable to open " << filename << " (%m)");

	char header[18];
	int ret = snprintf(header, sizeof(header), "%%PDF-%d.%d\r%%%c%c%c%c\r\n",
			   version_major, version_minor,
			   0xe2, 0xe3, 0xcf, 0xd3);
	
	writeBuffer(newFd, header, ret);

	int maxId = 0;
	std::stringstream xref;
	off_t xrefStmOffset = 0;
	
	xref << std::setfill('0');
	xref << "xref\n";
	xref << "0 1\n";
	xref << "0000000000 65535 f\r\n";
	
	std::vector<Object*>::iterator it;
	for(it=_objects.begin(); it!=_objects.end(); it++)
	{
	    Object* object = *it;
	    std::string objStr = object->str();
	    curOffset = lseek(newFd, 0, SEEK_CUR);
	    writeBuffer(newFd, objStr.c_str(), objStr.size());
	    xref << std::setw(0) << object->objectId() << " 1\n";
	    xref << std::setw(10) << curOffset << " " << std::setw(5) << object->generationNumber();
	    if (object->used())
		xref << " n";
	    else
		xref << " f" ; 
	    xref << "\r\n" ; // Here \r seems important

	    if (object->objectId() > maxId)
		maxId = object->objectId();

	    if (object->hasKey("Type") && (*object)["Type"]->str() == "/XRef")
	    {
		// Try to keep Prev link valid
		if (object->hasKey("Prev") && xrefStmOffset != 0)
		{
		    object->deleteKey("Prev");
		    object->dictionary().addData("Prev", new Integer(xrefStmOffset));
		}
		xrefStmOffset = curOffset;
	    }
	}

	off_t newXrefOffset = lseek(newFd, 0, SEEK_CUR);

	std::string xrefStr = xref.str();
	writeBuffer(newFd, xrefStr.c_str(), xrefStr.size());

	trailer.deleteKey("Prev");
	trailer.deleteKey("Size");
	trailer.dictionary().addData("Size", new Integer(maxId+1));

	trailer.deleteKey("XRefStm");
	if (xrefStmOffset != 0)
	    trailer.dictionary().addData("XRefStm", new Integer(xrefStmOffset));

	std::string trailerStr = trailer.dictionary().str();
	writeBuffer(newFd, "trailer\n", 8);
	writeBuffer(newFd, trailerStr.c_str(), trailerStr.size());

	std::stringstream startxref;
	startxref << "startxref\n" << newXrefOffset << "\n%%EOF";
	
	std::string startxrefStr = startxref.str();
	writeBuffer(newFd, startxrefStr.c_str(), startxrefStr.size());
	
	close(newFd);
    }
}
